// braw-bridge: Decode Blackmagic RAW files, output raw rgb24 video on stdout
// and NDJSON metadata/progress on stderr.
//
// Usage:
//   braw-bridge --input <file.braw> [--debayer full|half|quarter]
//   braw-bridge --input <file.braw> --extract-audio /path/to/output.wav
//

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <cmath>
#include <string>
#include <atomic>
#include <mutex>
#include <condition_variable>

#ifdef _WIN32
#include <io.h>
#include <fcntl.h>
#else
#include <unistd.h>
#endif

#include "LinuxCOM.h"
#include "BlackmagicRawAPI.h"

// ---------------------------------------------------------------------------
// Utility: write NDJSON to stderr
// ---------------------------------------------------------------------------

static void json_error(const char* msg)
{
    fprintf(stderr, "{\"type\":\"error\",\"message\":\"%s\"}\n", msg);
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
// WAV writer (for --extract-audio)
// ---------------------------------------------------------------------------

static bool write_wav(const char* path, const void* samples, uint64_t sample_count,
                      uint32_t sample_rate, uint32_t channels, uint32_t bits_per_sample)
{
    FILE* f = fopen(path, "wb");
    if (!f) return false;

    uint32_t byte_rate = sample_rate * channels * (bits_per_sample / 8);
    uint32_t block_align = channels * (bits_per_sample / 8);
    uint32_t data_size = (uint32_t)(sample_count * channels * (bits_per_sample / 8));
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
// BRAW Callback: processes frames asynchronously
// ---------------------------------------------------------------------------

class BrawCallback : public IBlackmagicRawCallback
{
public:
    BrawCallback(uint64_t total_frames, BlackmagicRawResolutionScale resolution_scale)
        : m_ref(1)
        , m_total_frames(total_frames)
        , m_completed_frames(0)
        , m_error(false)
        , m_resolution_scale(resolution_scale)
    {}

    // IUnknown
    virtual HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void**) override
    {
        return E_NOINTERFACE;
    }

    virtual ULONG STDMETHODCALLTYPE AddRef() override
    {
        return ++m_ref;
    }

    virtual ULONG STDMETHODCALLTYPE Release() override
    {
        ULONG ref = --m_ref;
        if (ref == 0) delete this;
        return ref;
    }

    // IBlackmagicRawCallback
    virtual void STDMETHODCALLTYPE ReadComplete(
        IBlackmagicRawJob* job, HRESULT result, IBlackmagicRawFrame* frame) override
    {
        if (FAILED(result))
        {
            json_error("ReadComplete failed");
            m_error = true;
            signal_frame_done();
            if (job) job->Release();
            return;
        }

        // Set pixel format and resolution scale before decoding
        frame->SetResourceFormat(blackmagicRawResourceFormatRGBAU8);
        if (m_resolution_scale != blackmagicRawResolutionScaleFull)
            frame->SetResolutionScale(m_resolution_scale);

        // Kick off decode+process for this frame
        IBlackmagicRawJob* decode_job = nullptr;
        HRESULT hr = frame->CreateJobDecodeAndProcessFrame(nullptr, nullptr, &decode_job);
        if (FAILED(hr) || !decode_job)
        {
            json_error("CreateJobDecodeAndProcessFrame failed");
            m_error = true;
            signal_frame_done();
            if (job) job->Release();
            return;
        }

        hr = decode_job->Submit();
        if (FAILED(hr))
        {
            json_error("Decode job submit failed");
            m_error = true;
            decode_job->Release();
            signal_frame_done();
        }

        if (job) job->Release();
    }

    virtual void STDMETHODCALLTYPE DecodeComplete(
        IBlackmagicRawJob* job, HRESULT result) override
    {
        // Stub — we use ProcessComplete for final output
        if (job) job->Release();
    }

    virtual void STDMETHODCALLTYPE ProcessComplete(
        IBlackmagicRawJob* job, HRESULT result,
        IBlackmagicRawProcessedImage* processed_image) override
    {
        if (FAILED(result) || !processed_image)
        {
            json_error("ProcessComplete failed");
            m_error = true;
            signal_frame_done();
            if (job) job->Release();
            return;
        }

        // Get pixel data from processed image
        uint32_t width = 0, height = 0;
        processed_image->GetWidth(&width);
        processed_image->GetHeight(&height);

        void* pixel_data = nullptr;
        processed_image->GetResource(&pixel_data);

        // We set RGBAU8 in ReadComplete, so layout is R, G, B, A per pixel.
        // Convert RGBA -> RGB24: drop alpha channel.
        size_t rgb_size = (size_t)width * height * 3;
        uint8_t* rgb_buf = (uint8_t*)malloc(rgb_size);
        if (rgb_buf && pixel_data)
        {
            const uint8_t* src = (const uint8_t*)pixel_data;
            uint8_t* dst = rgb_buf;
            size_t pixel_count = (size_t)width * height;
            for (size_t i = 0; i < pixel_count; i++)
            {
                dst[0] = src[0]; // R
                dst[1] = src[1]; // G
                dst[2] = src[2]; // B
                dst += 3;
                src += 4; // skip A
            }

            // Write raw rgb24 frame data to stdout
            fwrite(rgb_buf, 1, rgb_size, stdout);
            fflush(stdout);
        }
        else
        {
            json_error("Failed to allocate RGB buffer or null pixel data");
            m_error = true;
        }

        free(rgb_buf);

        m_completed_frames++;
        json_progress(m_completed_frames, m_total_frames);

        signal_frame_done();
        if (job) job->Release();
    }

    virtual void STDMETHODCALLTYPE TrimProgress(IBlackmagicRawJob*, float) override {}
    virtual void STDMETHODCALLTYPE TrimComplete(IBlackmagicRawJob*, HRESULT) override {}
    virtual void STDMETHODCALLTYPE SidecarMetadataParseWarning(
        IBlackmagicRawClip*, const char*, uint32_t, const char*) override {}
    virtual void STDMETHODCALLTYPE SidecarMetadataParseError(
        IBlackmagicRawClip*, const char*, uint32_t, const char*) override {}
    virtual void STDMETHODCALLTYPE PreparePipelineComplete(void*, HRESULT) override {}

    bool had_error() const { return m_error; }

    // Synchronization: wait for one frame to complete
    void wait_frame_done()
    {
        std::unique_lock<std::mutex> lock(m_mutex);
        m_cv.wait(lock, [this]{ return m_frame_done; });
        m_frame_done = false;
    }

private:
    void signal_frame_done()
    {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_frame_done = true;
        m_cv.notify_one();
    }

    std::atomic<ULONG> m_ref;
    BlackmagicRawResolutionScale m_resolution_scale;
    uint64_t m_total_frames;
    uint64_t m_completed_frames;
    std::atomic<bool> m_error;

    std::mutex m_mutex;
    std::condition_variable m_cv;
    bool m_frame_done = false;
};

// ---------------------------------------------------------------------------
// Timecode extraction helper
// ---------------------------------------------------------------------------

static std::string get_timecode(IBlackmagicRawClip* clip)
{
    const char* tc_str = nullptr;
    HRESULT hr = clip->GetTimecodeForFrame(0, &tc_str);
    if (SUCCEEDED(hr) && tc_str && tc_str[0] != '\0')
        return std::string(tc_str);
    return "00:00:00:00";
}

// ---------------------------------------------------------------------------
// Audio extraction (Phase 3)
// ---------------------------------------------------------------------------

static bool extract_audio(IBlackmagicRawClip* clip, const char* output_path)
{
    IBlackmagicRawClipAudio* audio = nullptr;
    HRESULT hr = clip->QueryInterface(IID_IBlackmagicRawClipAudio, (void**)&audio);
    if (FAILED(hr) || !audio)
    {
        json_error("No audio in BRAW clip");
        return false;
    }

    uint64_t sample_count = 0;
    uint32_t bits_per_sample = 0;
    uint32_t channel_count = 0;
    uint32_t sample_rate = 0;

    hr = audio->GetAudioSampleCount(&sample_count);
    if (FAILED(hr) || sample_count == 0)
    {
        json_error("No audio samples in BRAW clip");
        audio->Release();
        return false;
    }
    hr = audio->GetAudioBitDepth(&bits_per_sample);
    if (FAILED(hr)) { json_error("GetAudioBitDepth failed"); audio->Release(); return false; }
    hr = audio->GetAudioChannelCount(&channel_count);
    if (FAILED(hr)) { json_error("GetAudioChannelCount failed"); audio->Release(); return false; }
    hr = audio->GetAudioSampleRate(&sample_rate);
    if (FAILED(hr)) { json_error("GetAudioSampleRate failed"); audio->Release(); return false; }

    // Read in chunks of 48000 samples (as recommended by SDK samples)
    static constexpr uint32_t kChunkSamples = 48000;
    uint32_t chunk_buf_bytes = (kChunkSamples * channel_count * bits_per_sample) / 8;
    size_t total_data_bytes = (size_t)((sample_count * channel_count * bits_per_sample) / 8);

    uint8_t* audio_buffer = (uint8_t*)malloc(total_data_bytes);
    if (!audio_buffer)
    {
        json_error("Failed to allocate audio buffer");
        audio->Release();
        return false;
    }

    uint8_t* chunk_buf = (uint8_t*)malloc(chunk_buf_bytes);
    if (!chunk_buf)
    {
        json_error("Failed to allocate chunk buffer");
        free(audio_buffer);
        audio->Release();
        return false;
    }

    uint64_t sample_idx = 0;
    size_t buf_offset = 0;
    while (sample_idx < sample_count)
    {
        uint32_t samples_read = 0;
        uint32_t bytes_read = 0;
        hr = audio->GetAudioSamples((uint32_t)sample_idx, chunk_buf, chunk_buf_bytes,
                                     kChunkSamples, &samples_read, &bytes_read);
        if (FAILED(hr) || samples_read == 0)
            break;
        if (buf_offset + bytes_read <= total_data_bytes)
        {
            memcpy(audio_buffer + buf_offset, chunk_buf, bytes_read);
            buf_offset += bytes_read;
        }
        sample_idx += samples_read;
    }

    free(chunk_buf);
    audio->Release();

    bool ok = write_wav(output_path, audio_buffer, sample_idx,
                        sample_rate, channel_count, bits_per_sample);
    free(audio_buffer);

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
    BlackmagicRawResolutionScale resolution_scale = blackmagicRawResolutionScaleFull;
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
            if (strcmp(argv[i], "full") == 0)
                opts.resolution_scale = blackmagicRawResolutionScaleFull;
            else if (strcmp(argv[i], "half") == 0)
                opts.resolution_scale = blackmagicRawResolutionScaleHalf;
            else if (strcmp(argv[i], "quarter") == 0)
                opts.resolution_scale = blackmagicRawResolutionScaleQuarter;
            else
            {
                json_error("Invalid debayer option. Use: full, half, quarter");
                return false;
            }
        }
        else if (strcmp(argv[i], "--extract-audio") == 0 && i + 1 < argc)
        {
            opts.extract_audio_path = argv[++i];
        }
        else
        {
            fprintf(stderr, "Unknown argument: %s\n", argv[i]);
            json_error("Invalid arguments");
            return false;
        }
    }

    if (opts.input_file.empty())
    {
        json_error("Missing --input <file.braw>");
        return false;
    }

    return true;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

int main(int argc, char* argv[])
{
#ifdef _WIN32
    // Set stdout to binary mode on Windows
    _setmode(_fileno(stdout), _O_BINARY);
#endif

    Options opts;
    if (!parse_args(argc, argv, opts))
        return 1;

    // --- Initialize BRAW SDK ---

    // Resolve SDK library directory relative to executable via /proc/self/exe
    std::string lib_dir;
    {
        char exe_path[4096] = {};
        ssize_t len = readlink("/proc/self/exe", exe_path, sizeof(exe_path) - 1);
        if (len > 0)
        {
            exe_path[len] = '\0';
            std::string exe(exe_path);
            auto pos = exe.rfind('/');
            if (pos != std::string::npos)
                lib_dir = exe.substr(0, pos) + "/../sdk/Libraries/Linux";
            else
                lib_dir = "../sdk/Libraries/Linux";
        }
        else
        {
            lib_dir = "../sdk/Libraries/Linux";
        }
    }

    // CreateBlackmagicRawFactoryInstanceFromPath returns the factory directly
    IBlackmagicRawFactory* factory = CreateBlackmagicRawFactoryInstanceFromPath(lib_dir.c_str());
    if (!factory)
    {
        json_error("Failed to create BRAW factory. Is the SDK installed under braw-bridge/sdk/?");
        return 1;
    }

    IBlackmagicRaw* codec = nullptr;
    HRESULT hr = factory->CreateCodec(&codec);
    if (FAILED(hr) || !codec)
    {
        json_error("Failed to create BRAW codec");
        factory->Release();
        return 1;
    }

    // --- Open clip ---

    IBlackmagicRawClip* clip = nullptr;
    hr = codec->OpenClip(opts.input_file.c_str(), &clip);
    if (FAILED(hr) || !clip)
    {
        json_error("Failed to open BRAW clip");
        codec->Release();
        factory->Release();
        return 1;
    }

    // --- Get clip properties ---

    uint64_t frame_count = 0;
    clip->GetFrameCount(&frame_count);

    float frame_rate = 0.0f;
    clip->GetFrameRate(&frame_rate);

    // GetFrameRate returns float; convert to rational num/den
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
            if (fabsf(frame_rate - kr.rate) < 0.05f)
            {
                fps_num = kr.num;
                fps_den = kr.den;
                matched = true;
                break;
            }
        }

        if (!matched)
        {
            fps_num = (uint32_t)roundf(frame_rate);
            fps_den = 1;
        }
    }

    uint32_t width = 0, height = 0;
    clip->GetWidth(&width);
    clip->GetHeight(&height);

    // Adjust dimensions for debayer resolution scale
    if (opts.resolution_scale == blackmagicRawResolutionScaleHalf)
    {
        width /= 2;
        height /= 2;
    }
    else if (opts.resolution_scale == blackmagicRawResolutionScaleQuarter)
    {
        width /= 4;
        height /= 4;
    }

    // Timecode
    std::string timecode = get_timecode(clip);

    // --- Handle --extract-audio ---

    if (!opts.extract_audio_path.empty())
    {
        bool ok = extract_audio(clip, opts.extract_audio_path.c_str());

        clip->Release();
        codec->Release();
        factory->Release();

        if (ok)
        {
            json_done();
            return 0;
        }
        return 1;
    }

    // --- Emit metadata JSON (FIRST line on stderr) ---

    json_metadata(timecode.c_str(), fps_num, fps_den, width, height, frame_count);
    fflush(stderr);

    // --- Process frames ---

    BrawCallback* callback = new BrawCallback(frame_count, opts.resolution_scale);
    codec->SetCallback(callback);

    for (uint64_t frame_idx = 0; frame_idx < frame_count; frame_idx++)
    {
        IBlackmagicRawJob* read_job = nullptr;
        hr = clip->CreateJobReadFrame(frame_idx, &read_job);
        if (FAILED(hr) || !read_job)
        {
            json_error("CreateJobReadFrame failed");
            break;
        }

        hr = read_job->Submit();
        if (FAILED(hr))
        {
            json_error("ReadJob submit failed");
            read_job->Release();
            break;
        }

        // Wait for this frame to be fully processed before submitting next
        // This ensures frames are output in order on stdout
        callback->wait_frame_done();

        if (callback->had_error())
            break;
    }

    // --- Cleanup ---

    bool had_error = callback->had_error();

    codec->FlushJobs();

    codec->SetCallback(nullptr);
    callback->Release();
    clip->Release();
    codec->Release();
    factory->Release();

    if (!had_error)
    {
        json_done();
        return 0;
    }

    return 1;
}
