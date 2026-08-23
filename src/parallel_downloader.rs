// Segmented parallel downloader

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadChunk {
    pub chunk_index: usize,
    pub start_byte: u64,
    pub end_byte: u64,
    pub downloaded_bytes: u64,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelDownloadTask {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub total_size: u64,
    pub chunks: Vec<DownloadChunk>,
    pub num_connections: usize,
    pub is_finished: bool,
}

impl ParallelDownloadTask {
    pub fn new(
        id: String,
        url: String,
        file_name: String,
        total_size: u64,
        num_connections: usize,
    ) -> Self {
        // Never slice a file into more chunks than it has bytes: with fewer
        // bytes than connections, chunk_size would be zero and the
        // inclusive end-byte arithmetic below would underflow.
        let num_connections = {
            let requested = num_connections.clamp(1, 16);
            if total_size > 0 && (requested as u64) > total_size {
                total_size as usize
            } else {
                requested
            }
        };
        let num_connections = if total_size == 0 { 1 } else { num_connections };
        let chunk_size = if total_size > 0 {
            total_size / num_connections as u64
        } else {
            0
        };

        let mut chunks = Vec::new();
        for i in 0..num_connections {
            let start_byte = i as u64 * chunk_size;
            let end_byte = if i == num_connections - 1 {
                total_size.saturating_sub(1)
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            chunks.push(DownloadChunk {
                chunk_index: i,
                start_byte,
                end_byte,
                downloaded_bytes: 0,
                completed: false,
            });
        }

        Self {
            id,
            url,
            file_name,
            total_size,
            chunks,
            num_connections,
            is_finished: false,
        }
    }

    pub fn progress_percentage(&self) -> f32 {
        if self.total_size == 0 {
            return 0.0;
        }
        let total_downloaded: u64 = self.chunks.iter().map(|c| c.downloaded_bytes).sum();
        (total_downloaded as f32 / self.total_size as f32) * 100.0
    }

    pub fn mark_chunk_complete(&mut self, chunk_index: usize) {
        if let Some(chunk) = self.chunks.get_mut(chunk_index) {
            chunk.completed = true;
            chunk.downloaded_bytes = chunk.end_byte.saturating_sub(chunk.start_byte) + 1;
        }
        self.is_finished = self.chunks.iter().all(|c| c.completed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_download_chunks() {
        let mut task = ParallelDownloadTask::new(
            "task-1".to_string(),
            "https://example.com/file.zip".to_string(),
            "file.zip".to_string(),
            1000,
            4,
        );

        assert_eq!(task.chunks.len(), 4);
        assert_eq!(task.chunks[0].start_byte, 0);
        assert_eq!(task.chunks[0].end_byte, 249);
        assert_eq!(task.chunks[3].end_byte, 999);

        assert_eq!(task.progress_percentage(), 0.0);
        task.mark_chunk_complete(0);
        assert!(task.progress_percentage() > 20.0);
    }
}
