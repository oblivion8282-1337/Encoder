// r3d-bridge: Decode RED R3D files, output raw rgb24 video on stdout
// and NDJSON metadata/progress on stderr.
//
// Usage:
//   r3d-bridge --input <file.R3D> [--debayer premium|half|quarter|eighth]
//   r3d-bridge --input <file.R3D> --extract-audio /path/to/output.wav
//   r3d-bridge --input <file.R3D> --probe-only
//

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <string>

#include <unistd.h>

#include "R3DSDK.h"

// ---------------------------------------------------------------------------
// Utility: write NDJSON to stderr
// ---------------------------------------------------------------------------

static std::string json_escape(const char* s)
{
    std::string out;
    for (; *s; ++s)
    {
        switch (*s)
        {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n";  break;
            case '\r': out += "\\r";  break;
            case '\t': out += "\\t";  break;
            default:   out += *s;     break;
        }
    }
    return out;
}

static void json_error(const char* msg)
{
    std::string escaped = json_escape(msg);
    fprintf(stderr, "{\"type\":\"error\",\"message\":\"%s\"}\n", escaped.c_str());
}

static void json_metadata(const char* timecode, uint32_t fps_num, uint32_t fps_den,
                           uint32_t width, uint32_t height, uint64_t frame_count)
{
    fprintf(stderr,
        "{\"type\":\"metadata\","
        "\"timecode\":\"%s\","
        "\"fps_num\":%u,"
        "\"fps_den\":%u,"
        "\"width\":%u,"
        "\"height\":%u,"
        "\"frame_count\":%llu}\n",
        timecode, fps_num, fps_den, width, height,
        (unsigned long long)frame_count);
}

static void json_progress(uint64_t frame, uint64_t total)
{
    fprintf(stderr, "{\"type\":\"progress\",\"frame\":%llu,\"total\":%llu}\n",
        (unsigned long long)frame, (unsigned long long)total);
}

static void json_done()
{
    fprintf(stderr, "{\"type\":\"done\"}\n");
}

// ---------------------------------------------------------------------------
// WAV writer
// ---------------------------------------------------------------------------

static bool write_wav(const char* path, const void* samples, uint64_t sample_count,
                      uint32_t sample_rate, uint32_t channels, uint32_t bits_per_sample)
{
    FILE* f = fopen(path, "wb");
    if (!f) return false;

    uint32_t byte_rate = sample_rate * channels * (bits_per_sample / 8);
    uint32_t block_align = channels * (bits_per_sample / 8);
    uint64_t data_size_64 = (uint64_t)sample_count * channels * (bits_per_sample / 8);
    if (data_size_64 > 0xFFFFFFFFULL)
    {
        json_error("Audio data too large for WAV format (exceeds 4 GiB)");
        fclose(f);
        return false;
    }
    uint32_t data_size = (uint32_t)data_size_64;
    uint32_t riff_size = 36 + data_size;

    // RIFF header
    fwrite("RIFF", 1, 4, f);
    fwrite(&riff_size, 4, 1, f);
    fwrite("WAVE", 1, 4, f);

    // fmt chunk
    fwrite("fmt ", 1, 4, f);
    uint32_t fmt_size = 16;
    uint16_t audio_format = 1; // PCM
    uint16_t ch = (uint16_t)channels;
    uint16_t bps = (uint16_t)bits_per_sample;
    fwrite(&fmt_size, 4, 1, f);
    fwrite(&audio_format, 2, 1, f);
    fwrite(&ch, 2, 1, f);
    fwrite(&sample_rate, 4, 1, f);
    fwrite(&byte_rate, 4, 1, f);
    fwrite(&block_align, 2, 1, f);
    fwrite(&bps, 2, 1, f);

    // data chunk
    fwrite("data", 1, 4, f);
    fwrite(&data_size, 4, 1, f);
    fwrite(samples, 1, data_size, f);

    fclose(f);
    return true;
}

// ---------------------------------------------------------------------------
// Aligned malloc: 512-byte alignment (required by R3D SDK for audio)
// ---------------------------------------------------------------------------

struct AlignedBuffer
{
    uint8_t* base;   // original pointer from malloc (for free)
    uint8_t* ptr;    // 512-byte aligned pointer
    size_t   offset; // ptr - base

    AlignedBuffer() : base(nullptr), ptr(nullptr), offset(0) {}

    bool alloc(size_t size)
    {
        base = (uint8_t*)malloc(size + 511);
        if (!base) return false;
        uintptr_t addr = (uintptr_t)base;
        offset = (addr % 512 == 0) ? 0 : (512 - addr % 512);
        ptr = base + offset;
        return true;
    }

    void free_buf()
    {
        if (base) { free(base); base = nullptr; ptr = nullptr; }
    }
};

// ---------------------------------------------------------------------------
// SDK library path resolution
// ---------------------------------------------------------------------------

static std::string find_sdk_lib_dir()
{
    // 1. Environment variable override
    const char* env = getenv("R3D_SDK_LIB_PATH");
    if (env && env[0] != '\0')
        return std::string(env);

    // 2. Relative to executable via /proc/self/exe
    char exe_path[4096] = {};
    ssize_t len = readlink("/proc/self/exe", exe_path, sizeof(exe_path) - 1);
    if (len > 0)
    {
        exe_path[len] = '\0';
        std::string exe(exe_path);
        auto pos = exe.rfind('/');
        if (pos != std::string::npos)
            return exe.substr(0, pos) + "/../R3DSDKv9_1_2/Redistributable/linux";
    }

    // 3. Current directory fallback
    return "./R3DSDKv9_1_2/Redistributable/linux";
}

// ---------------------------------------------------------------------------
// Audio extraction
// ---------------------------------------------------------------------------

static bool extract_audio(R3DSDK::Clip* clip, const char* output_path)
{
    size_t max_block_size = 0;
    size_t blocks = clip->AudioBlockCountAndSize(&max_block_size);

    if (blocks == 0 || max_block_size == 0)
    {
        json_error("No audio in R3D clip");
        return false;
    }

    size_t channels = clip->AudioChannelCount();
    if (channels == 0)
    {
        json_error("No audio channels in R3D clip");
        return false;
    }

    uint32_t sample_rate = clip->MetadataItemAsInt(R3DSDK::RMD_SAMPLERATE);
    if (sample_rate == 0) sample_rate = 48000;

    // SDK always delivers 4-byte (32-bit) words per sample regardless of the recorded
    // bit depth (24 or 32). We write 32-bit signed LE PCM to WAV; FFmpeg reads pcm_s32le.
    const uint32_t wav_bits = 32;
    const size_t bytes_per_sample = 4;

    // Total sample count per channel
    unsigned long long total_samples = clip->AudioSampleCount();
    if (total_samples == 0)
    {
        json_error("No audio samples in R3D clip");
        return false;
    }

    // Allocate output buffer (all audio in memory)
    uint64_t total_bytes = (uint64_t)total_samples * channels * bytes_per_sample;
    if (total_bytes > 0xFFFFFFFFULL)
    {
        json_error("Audio data too large for WAV format (exceeds 4 GiB)");
        return false;
    }

    uint8_t* audio_out = (uint8_t*)malloc((size_t)total_bytes);
    if (!audio_out)
    {
        json_error("Failed to allocate audio output buffer");
        return false;
    }

    // Allocate 512-byte aligned block buffer for decoding
    AlignedBuffer block_buf;
    if (!block_buf.alloc(max_block_size))
    {
        json_error("Failed to allocate audio block buffer");
        free(audio_out);
        return false;
    }

    size_t out_offset = 0;

    for (size_t bl = 0; bl < blocks; bl++)
    {
        size_t buf_size = max_block_size;
        R3DSDK::DecodeStatus ds = clip->DecodeAudioBlock(bl, block_buf.ptr, &buf_size);
        if (ds != R3DSDK::DSDecodeOK || buf_size == 0)
            break;

        // Byte-swap: R3D audio is Big Endian 32-bit; swap to Little Endian
        uint8_t* p = block_buf.ptr;
        for (size_t i = 0; i + 3 < buf_size; i += 4)
        {
            uint8_t b0 = p[i], b1 = p[i+1], b2 = p[i+2], b3 = p[i+3];
            p[i]   = b3;
            p[i+1] = b2;
            p[i+2] = b1;
            p[i+3] = b0;
        }

        size_t copy_bytes = buf_size;
        if (out_offset + copy_bytes > (size_t)total_bytes)
            copy_bytes = (size_t)total_bytes - out_offset;

        memcpy(audio_out + out_offset, block_buf.ptr, copy_bytes);
        out_offset += copy_bytes;
    }

    block_buf.free_buf();

    bool ok = write_wav(output_path, audio_out, total_samples,
                        sample_rate, (uint32_t)channels, wav_bits);
    free(audio_out);

    if (!ok)
    {
        json_error("Failed to write WAV file");
        return false;
    }

    return true;
}

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

struct Options
{
    std::string input_file;
    std::string extract_audio_path;
    R3DSDK::VideoDecodeMode decode_mode = R3DSDK::DECODE_HALF_RES_GOOD;
    bool probe_only = false;
};

static bool parse_args(int argc, char* argv[], Options& opts)
{
    for (int i = 1; i < argc; i++)
    {
        if ((strcmp(argv[i], "--input") == 0 || strcmp(argv[i], "-i") == 0) && i + 1 < argc)
        {
            opts.input_file = argv[++i];
        }
        else if (strcmp(argv[i], "--debayer") == 0 && i + 1 < argc)
        {
            i++;
            if (strcmp(argv[i], "premium") == 0)
                opts.decode_mode = R3DSDK::DECODE_FULL_RES_PREMIUM;
            else if (strcmp(argv[i], "half") == 0)
                opts.decode_mode = R3DSDK::DECODE_HALF_RES_GOOD;
            else if (strcmp(argv[i], "quarter") == 0)
                opts.decode_mode = R3DSDK::DECODE_QUARTER_RES_GOOD;
            else if (strcmp(argv[i], "eighth") == 0)
                opts.decode_mode = R3DSDK::DECODE_EIGHT_RES_GOOD;
            else
            {
                json_error("Invalid debayer option. Use: premium, half, quarter, eighth");
                return false;
            }
        }
        else if (strcmp(argv[i], "--extract-audio") == 0 && i + 1 < argc)
        {
            opts.extract_audio_path = argv[++i];
        }
        else if (strcmp(argv[i], "--probe-only") == 0)
        {
            opts.probe_only = true;
        }
        else
        {
            char msg[256];
            snprintf(msg, sizeof(msg), "Unknown argument: %s", argv[i]);
            json_error(msg);
            return false;
        }
    }

    if (opts.input_file.empty())
    {
        json_error("Missing --input <file.R3D>");
        return false;
    }

    return true;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

int main(int argc, char* argv[])
{
    Options opts;
    if (!parse_args(argc, argv, opts))
        return 1;

    // --- Initialize R3D SDK ---

    std::string lib_dir = find_sdk_lib_dir();

    R3DSDK::InitializeStatus init_status = R3DSDK::InitializeSdk(lib_dir.c_str(), OPTION_RED_NONE);
    if (init_status != R3DSDK::ISInitializeOK)
    {
        char msg[512];
        snprintf(msg, sizeof(msg),
            "Failed to initialize R3D SDK (status=%d). "
            "Dynamic libraries not found at: %s",
            (int)init_status, lib_dir.c_str());
        json_error(msg);
        return 1;
    }

    // --- Open clip ---

    R3DSDK::Clip* clip = new R3DSDK::Clip(opts.input_file.c_str());

    if (clip->Status() != R3DSDK::LSClipLoaded)
    {
        char msg[512];
        snprintf(msg, sizeof(msg), "Failed to open R3D clip (status=%d): %s",
            (int)clip->Status(), opts.input_file.c_str());
        json_error(msg);
        delete clip;
        R3DSDK::FinalizeSdk();
        return 1;
    }

    // --- Get clip properties ---

    size_t full_width  = clip->Width();
    size_t full_height = clip->Height();
    size_t frame_count = clip->VideoFrameCount();

    if (full_width == 0 || full_height == 0 || frame_count == 0)
    {
        json_error("R3D clip has zero width, height or frames");
        delete clip;
        R3DSDK::FinalizeSdk();
        return 1;
    }

    // Frame rate → rational
    float fps_float = clip->VideoAudioFramerate();
    uint32_t fps_num = 0, fps_den = 1;
    {
        struct { float rate; uint32_t num; uint32_t den; } known_rates[] = {
            {23.976f,  24000, 1001},
            {24.0f,    24,    1},
            {25.0f,    25,    1},
            {29.97f,   30000, 1001},
            {30.0f,    30,    1},
            {47.952f,  48000, 1001},
            {48.0f,    48,    1},
            {50.0f,    50,    1},
            {59.94f,   60000, 1001},
            {60.0f,    60,    1},
            {119.88f,  120000, 1001},
            {120.0f,   120,   1},
        };

        bool matched = false;
        for (auto& kr : known_rates)
        {
            if (fabsf(fps_float - kr.rate) < 0.05f)
            {
                fps_num = kr.num;
                fps_den = kr.den;
                matched = true;
                break;
            }
        }
        if (!matched)
        {
            fps_num = (uint32_t)roundf(fps_float);
            fps_den = 1;
        }
    }

    // Timecode: try AbsoluteTimecode first, fall back to Timecode
    std::string timecode = "00:00:00:00";
    {
        const char* tc = clip->AbsoluteTimecode(0);
        if (tc && tc[0] != '\0')
            timecode = tc;
        else
        {
            tc = clip->Timecode(0);
            if (tc && tc[0] != '\0')
                timecode = tc;
        }
    }

    // Dimensions after debayer
    size_t out_width  = full_width;
    size_t out_height = full_height;
    switch (opts.decode_mode)
    {
        case R3DSDK::DECODE_HALF_RES_GOOD:
        case R3DSDK::DECODE_HALF_RES_PREMIUM:
            out_width  /= 2;
            out_height /= 2;
            break;
        case R3DSDK::DECODE_QUARTER_RES_GOOD:
            out_width  /= 4;
            out_height /= 4;
            break;
        case R3DSDK::DECODE_EIGHT_RES_GOOD:
            out_width  /= 8;
            out_height /= 8;
            break;
        default:
            break;
    }

    // --- Handle --extract-audio ---

    if (!opts.extract_audio_path.empty())
    {
        bool ok = extract_audio(clip, opts.extract_audio_path.c_str());
        delete clip;
        R3DSDK::FinalizeSdk();
        if (ok)
        {
            json_done();
            return 0;
        }
        return 1;
    }

    // --- Emit metadata JSON ---

    json_metadata(timecode.c_str(), fps_num, fps_den,
                  (uint32_t)out_width, (uint32_t)out_height, (uint64_t)frame_count);
    fflush(stderr);

    if (opts.probe_only)
    {
        delete clip;
        R3DSDK::FinalizeSdk();
        return 0;
    }

    // --- Allocate frame buffer (16-byte aligned) ---

    size_t frame_bytes = out_width * out_height * 3; // 3 bytes per pixel (BGR→RGB)
    void* frame_buf_raw = nullptr;
    if (posix_memalign(&frame_buf_raw, 16, frame_bytes) != 0 || !frame_buf_raw)
    {
        json_error("Failed to allocate frame buffer");
        delete clip;
        R3DSDK::FinalizeSdk();
        return 1;
    }
    uint8_t* frame_buf = (uint8_t*)frame_buf_raw;

    // --- Frame decode loop ---

    R3DSDK::VideoDecodeJob job;
    job.Mode             = opts.decode_mode;
    job.PixelType        = R3DSDK::PixelType_8Bit_BGR_Interleaved;
    job.OutputBuffer     = frame_buf;
    job.OutputBufferSize = frame_bytes;

    bool had_error = false;

    for (size_t i = 0; i < frame_count; i++)
    {
        R3DSDK::DecodeStatus ds = clip->DecodeVideoFrame(i, job);
        if (ds != R3DSDK::DSDecodeOK)
        {
            char msg[128];
            snprintf(msg, sizeof(msg), "DecodeVideoFrame failed at frame %zu (status=%d)", i, (int)ds);
            json_error(msg);
            had_error = true;
            break;
        }

        // BGR → RGB: swap R and B channels in-place
        for (size_t px = 0; px < out_width * out_height; px++)
        {
            uint8_t b = frame_buf[px * 3 + 0];
            frame_buf[px * 3 + 0] = frame_buf[px * 3 + 2]; // R ← B
            frame_buf[px * 3 + 2] = b;                      // B ← R
        }

        fwrite(frame_buf, 1, frame_bytes, stdout);
        fflush(stdout);

        json_progress((uint64_t)(i + 1), (uint64_t)frame_count);
    }

    // --- Cleanup ---

    free(frame_buf_raw);
    delete clip;
    R3DSDK::FinalizeSdk();

    if (!had_error)
    {
        json_done();
        return 0;
    }

    return 1;
}
