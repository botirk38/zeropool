/// File Processing Pipeline Profiling Program
///
/// Simulates a file processing pipeline with Read → Transform → Write stages.
/// This workload demonstrates:
/// - Sequential file processing with buffer reuse
/// - Varied chunk sizes (4KB - 1MB) for different file types
/// - Multi-stage pipeline showing buffer handoff patterns
/// - Realistic I/O workload with data transformations
///
/// Usage: cargo flamegraph --profile profiling --bin file_pipeline

use std::time::Instant;
use zeropool::BufferPool;

// Configuration - increased for better profiling visibility
const NUM_FILES: usize = 500;  // Increased from 100
const CHUNKS_PER_FILE: usize = 50;
const SIMULATION_RUNS: usize = 3;

// File types with different characteristics
#[derive(Clone, Copy)]
enum FileType {
    SmallText,      // 4KB chunks - logs, configs
    MediumJson,     // 16KB chunks - structured data
    LargeImage,     // 64KB chunks - images
    VideoChunk,     // 256KB chunks - video processing
    Database,       // 1MB chunks - database dumps
}

impl FileType {
    fn chunk_size(&self) -> usize {
        match self {
            FileType::SmallText => 4 * 1024,
            FileType::MediumJson => 16 * 1024,
            FileType::LargeImage => 64 * 1024,
            FileType::VideoChunk => 256 * 1024,
            FileType::Database => 1024 * 1024,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            FileType::SmallText => "text",
            FileType::MediumJson => "json",
            FileType::LargeImage => "image",
            FileType::VideoChunk => "video",
            FileType::Database => "database",
        }
    }

    fn all() -> &'static [FileType] {
        &[
            FileType::SmallText,
            FileType::MediumJson,
            FileType::LargeImage,
            FileType::VideoChunk,
            FileType::Database,
        ]
    }
}

fn main() {
    eprintln!("=== File Processing Pipeline Profiling ===");
    eprintln!("Files to process: {}", NUM_FILES);
    eprintln!("Chunks per file: {}", CHUNKS_PER_FILE);
    eprintln!("Simulation runs: {}", SIMULATION_RUNS);
    eprintln!();

    // Create pool optimized for file I/O
    let pool = BufferPool::builder()
        .num_shards(8)
        .tls_cache_size(4)
        .max_buffers_per_shard(24)
        .min_buffer_size(4 * 1024)
        .build();

    eprintln!("Pre-allocating buffers...");
    preallocate_buffers(&pool);

    let total_start = Instant::now();

    for run in 1..=SIMULATION_RUNS {
        let run_start = Instant::now();
        eprintln!("\n[Run {}/{}] Processing files...", run, SIMULATION_RUNS);

        let bytes_processed = process_all_files(&pool);

        let run_duration = run_start.elapsed();
        eprintln!("[Run {}/{}] Completed in {:.2?}, {:.2} GB processed",
                 run, SIMULATION_RUNS, run_duration, bytes_processed as f64 / 1e9);
    }

    let total_duration = total_start.elapsed();
    eprintln!("\n=== Profiling Complete ===");
    eprintln!("Total time: {:.2?}", total_duration);
    eprintln!("Average run time: {:.2?}", total_duration / SIMULATION_RUNS as u32);
    eprintln!("Pool stats: {} buffers in pool", pool.len());
}

/// Pre-allocate common buffer sizes for different file types
fn preallocate_buffers(pool: &BufferPool) {
    for file_type in FileType::all() {
        pool.preallocate(8, file_type.chunk_size());
    }
}

/// Process all files
fn process_all_files(pool: &BufferPool) -> usize {
    let mut total_bytes = 0;

    for file_idx in 0..NUM_FILES {
        let file_type = select_file_type(file_idx);
        total_bytes += process_file(pool, file_type);
    }

    total_bytes
}

/// Select file type based on index (distribute evenly)
fn select_file_type(index: usize) -> FileType {
    FileType::all()[index % FileType::all().len()]
}

/// Process a single file through the pipeline
fn process_file(pool: &BufferPool, file_type: FileType) -> usize {
    let chunk_size = file_type.chunk_size();
    let mut total_bytes = 0;

    for _ in 0..CHUNKS_PER_FILE {
        // Stage 1: Read chunk from disk
        let mut read_buffer = pool.get(chunk_size);
        read_chunk(&mut read_buffer);
        total_bytes += read_buffer.len();

        // Stage 2: Transform the data
        // In a real pipeline, this might be a separate buffer
        // Here we simulate in-place transformation
        transform_data(&mut read_buffer, file_type);

        // Stage 3: Write to output
        // We get a new buffer to simulate output buffering
        let mut write_buffer = pool.get(chunk_size);
        prepare_output(&read_buffer, &mut write_buffer);
        write_chunk(&write_buffer);
        total_bytes += write_buffer.len();

        // Buffers automatically returned to pool when dropped
    }

    total_bytes
}

/// Simulate reading a chunk from disk
fn read_chunk(buffer: &mut [u8]) {
    // Minimal work - just touch the buffer
    if !buffer.is_empty() {
        buffer[0] = 1;
    }
}

/// Transform data based on file type
fn transform_data(buffer: &mut [u8], _file_type: FileType) {
    // Minimal work - just touch the buffer
    std::hint::black_box(buffer.len());
}

/// Prepare output buffer (may involve compression, encryption, etc.)
fn prepare_output(input: &[u8], output: &mut [u8]) {
    // Minimal work - just touch both buffers
    std::hint::black_box(input.len());
    if !output.is_empty() {
        output[0] = 1;
    }
}

/// Simulate writing chunk to disk
fn write_chunk(buffer: &[u8]) {
    // Minimal work
    std::hint::black_box(buffer.len());
}
