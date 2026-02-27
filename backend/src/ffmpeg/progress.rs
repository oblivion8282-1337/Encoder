// Parst FFmpeg -progress pipe:2 Ausgabe (key=value Format auf stderr).
// Sammelt Bloecke bis "progress=continue" oder "progress=end".

use std::collections::HashMap;

/// Strukturierter Fortschritt aus einem FFmpeg -progress Block.
#[derive(Debug, Clone, Default)]
pub struct FfmpegProgress {
    pub frame: u64,
    pub fps: f32,
    pub speed: f32,
    pub out_time_us: i64,
    pub total_size: i64,
    pub is_done: bool,
}

/// Sammelt key=value Zeilen und produziert FfmpegProgress wenn ein Block komplett ist.
pub struct ProgressParser {
    current_block: HashMap<String, String>,
}

impl ProgressParser {
    pub fn new() -> Self {
        Self {
            current_block: HashMap::new(),
        }
    }

    /// Fuettert eine einzelne Zeile aus stderr.
    /// Gibt Some(FfmpegProgress) zurueck wenn ein Block abgeschlossen ist.
    pub fn feed_line(&mut self, line: &str) -> Option<FfmpegProgress> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        let (key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => return None,
        };

        let key = key.trim();
        let value = value.trim();

        if key == "progress" {
            let progress = self.build_progress(value == "end");
            self.current_block.clear();
            return Some(progress);
        }

        self.current_block.insert(key.to_string(), value.to_string());
        None
    }

    fn build_progress(&self, is_done: bool) -> FfmpegProgress {
        FfmpegProgress {
            frame: self.parse_u64("frame"),
            fps: self.parse_f32("fps"),
            speed: self.parse_speed(),
            out_time_us: self.parse_i64("out_time_us"),
            total_size: self.parse_i64("total_size"),
            is_done,
        }
    }

    fn parse_u64(&self, key: &str) -> u64 {
        self.current_block
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    fn parse_i64(&self, key: &str) -> i64 {
        self.current_block
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    fn parse_f32(&self, key: &str) -> f32 {
        self.current_block
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }

    /// speed wird als z.B. "1.23x" geliefert – das 'x' muss entfernt werden.
    fn parse_speed(&self) -> f32 {
        self.current_block
            .get("speed")
            .map(|v| v.trim_end_matches('x'))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0)
    }
}

/// Berechnet den Fortschritt (0.0 – 1.0) anhand der aktuellen
/// und der Gesamt-Dauer.
pub fn calculate_progress(current_time_us: i64, total_duration_us: i64) -> f32 {
    if total_duration_us <= 0 {
        return 0.0;
    }
    (current_time_us as f32 / total_duration_us as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_complete_block() {
        let mut parser = ProgressParser::new();
        assert!(parser.feed_line("frame=100").is_none());
        assert!(parser.feed_line("fps=25.0").is_none());
        assert!(parser.feed_line("speed=1.5x").is_none());
        assert!(parser.feed_line("out_time_us=4000000").is_none());
        assert!(parser.feed_line("total_size=1024000").is_none());

        let p = parser.feed_line("progress=continue").unwrap();
        assert_eq!(p.frame, 100);
        assert!((p.fps - 25.0).abs() < f32::EPSILON);
        assert!((p.speed - 1.5).abs() < f32::EPSILON);
        assert_eq!(p.out_time_us, 4_000_000);
        assert!(!p.is_done);
    }

    #[test]
    fn parse_end_block() {
        let mut parser = ProgressParser::new();
        assert!(parser.feed_line("frame=200").is_none());
        let p = parser.feed_line("progress=end").unwrap();
        assert_eq!(p.frame, 200);
        assert!(p.is_done);
    }

    #[test]
    fn progress_clamps_to_one() {
        assert_eq!(calculate_progress(120_000_000, 100_000_000), 1.0);
    }

    #[test]
    fn progress_zero_duration() {
        assert_eq!(calculate_progress(10_000_000, 0), 0.0);
    }

    #[test]
    fn progress_halfway() {
        let p = calculate_progress(50_000_000, 100_000_000);
        assert!((p - 0.5).abs() < f32::EPSILON);
    }
}
