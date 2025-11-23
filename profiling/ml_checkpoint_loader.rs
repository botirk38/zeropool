//! ML Checkpoint Loader Profiling Program
//!
//! Simulates loading large machine learning model checkpoints with realistic
//! buffer allocation patterns. This workload demonstrates:
//! - Large sequential allocations (tensor weights)
//! - Mixed small/large buffer sizes (metadata + tensors)
//! - Buffer reuse across multiple epochs
//! - Realistic ML workload from the README use case
//!
//! Usage: cargo flamegraph --profile profiling --bin ml_checkpoint_loader
use std::time::Instant;
use zeropool::BufferPool;
#[allow(missing_docs)]
// Configuration - massively increased for perf profiling
const TOTAL_MODEL_SIZE_GB: usize = 2;
const NUM_EPOCHS: usize = 1000; // Increased to 1000 for perf profiling
const NUM_LAYERS: usize = 48;
const METADATA_ITEMS_PER_LAYER: usize = 10;

// Buffer sizes (realistic for large language models)
const METADATA_SIZE: usize = 4 * 1024; // 4KB per metadata item
const EMBEDDING_WEIGHT_SIZE: usize = 128 * 1024 * 1024; // 128MB
const ATTENTION_WEIGHT_SIZE: usize = 64 * 1024 * 1024; // 64MB
const FFN_WEIGHT_SIZE: usize = 256 * 1024 * 1024; // 256MB
const LAYER_NORM_SIZE: usize = 1024 * 1024; // 1MB

/// Main function
fn main() {
    eprintln!("=== ML Checkpoint Loader Profiling ===");
    eprintln!("Model size: {TOTAL_MODEL_SIZE_GB} GB");
    eprintln!("Epochs: {NUM_EPOCHS}");
    eprintln!("Layers: {NUM_LAYERS}");
    eprintln!();

    // Create pool with configuration optimized for large buffers
    let pool = BufferPool::builder()
        .num_shards(8)
        .tls_cache_size(4)
        .max_buffers_per_shard(32)
        .min_buffer_size(1024 * 1024) // 1MB minimum
        .build();

    eprintln!("Pre-allocating buffers...");
    preallocate_buffers(&pool);

    let total_start = Instant::now();

    for epoch in 1..=NUM_EPOCHS {
        // Only print progress every 10 epochs to reduce I/O overhead
        if epoch % 10 == 1 || epoch == NUM_EPOCHS {
            eprintln!("Processing epoch {epoch}/{NUM_EPOCHS}...");
        }

        load_checkpoint(&pool);
    }

    let total_duration = total_start.elapsed();
    eprintln!("\n=== Profiling Complete ===");
    eprintln!("Total time: {total_duration:.2?}");
    eprintln!("Average epoch time: {:.2?}", total_duration / NUM_EPOCHS as u32);
    eprintln!("Pool stats: {} buffers in pool", pool.len());
}

/// Pre-allocate common buffer sizes to warm up the pool
fn preallocate_buffers(pool: &BufferPool) {
    // Pre-allocate some common sizes
    pool.preallocate(8, EMBEDDING_WEIGHT_SIZE);
    pool.preallocate(16, ATTENTION_WEIGHT_SIZE);
    pool.preallocate(16, FFN_WEIGHT_SIZE);
    pool.preallocate(32, METADATA_SIZE);
}

/// Simulate loading a complete model checkpoint
fn load_checkpoint(pool: &BufferPool) {
    // Load embedding layer
    load_embeddings(pool);

    // Load transformer layers
    for layer_idx in 0..NUM_LAYERS {
        load_transformer_layer(pool, layer_idx);
    }

    // Load final layer norm and output head
    load_output_head(pool);
}

/// Load embedding weights
fn load_embeddings(pool: &BufferPool) {
    // Token embeddings
    let mut embedding = pool.get(EMBEDDING_WEIGHT_SIZE);
    simulate_disk_read(&mut embedding);
    process_weights(&embedding);

    // Position embeddings
    let mut pos_embedding = pool.get(EMBEDDING_WEIGHT_SIZE / 2);
    simulate_disk_read(&mut pos_embedding);
    process_weights(&pos_embedding);

    // Buffers automatically returned to pool when dropped
}

/// Load a single transformer layer
fn load_transformer_layer(pool: &BufferPool, _layer_idx: usize) {
    // Load metadata first (config, shape info, etc.)
    for _ in 0..METADATA_ITEMS_PER_LAYER {
        let mut meta = pool.get(METADATA_SIZE);
        simulate_disk_read(&mut meta);
        parse_metadata(&meta);
    }

    // Self-attention weights (Q, K, V, O)
    for _ in 0..4 {
        let mut attn_weight = pool.get(ATTENTION_WEIGHT_SIZE);
        simulate_disk_read(&mut attn_weight);
        process_weights(&attn_weight);
    }

    // FFN weights (up_proj, down_proj)
    for _ in 0..2 {
        let mut ffn_weight = pool.get(FFN_WEIGHT_SIZE);
        simulate_disk_read(&mut ffn_weight);
        process_weights(&ffn_weight);
    }

    // Layer norms
    for _ in 0..2 {
        let mut norm_weight = pool.get(LAYER_NORM_SIZE);
        simulate_disk_read(&mut norm_weight);
        process_weights(&norm_weight);
    }
}

/// Load output head
fn load_output_head(pool: &BufferPool) {
    let mut final_norm = pool.get(LAYER_NORM_SIZE);
    simulate_disk_read(&mut final_norm);
    process_weights(&final_norm);

    let mut lm_head = pool.get(EMBEDDING_WEIGHT_SIZE);
    simulate_disk_read(&mut lm_head);
    process_weights(&lm_head);
}

/// Simulate reading data from disk into buffer
fn simulate_disk_read(buffer: &mut [u8]) {
    // Minimal fill - just touch the buffer
    // This focuses profiling on zeropool operations
    if !buffer.is_empty() {
        buffer[0] = 1;
        if buffer.len() > 1 {
            buffer[buffer.len() - 1] = 1;
        }
    }
}

/// Simulate processing/validating tensor weights
fn process_weights(buffer: &[u8]) {
    // Minimal work - just touch the buffer to prevent optimization
    // This makes allocation/deallocation visible in flamegraph
    std::hint::black_box(buffer.len());
    std::hint::black_box(&buffer[0]);
}

/// Simulate parsing metadata
fn parse_metadata(buffer: &[u8]) {
    // Minimal work - just touch the buffer
    std::hint::black_box(buffer.len());
}
