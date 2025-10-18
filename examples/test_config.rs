use zeropool::{BufferPool, PoolConfig};

fn main() {
    let pool = BufferPool::new();
    println!("Default pool configuration:");
    println!("  num_shards: {}", pool.config.num_shards);
    println!("  tls_cache_size: {}", pool.config.tls_cache_size);
    println!("  max_buffers_per_shard: {}", pool.config.max_buffers_per_shard);
    println!("  min_buffer_size: {} bytes", pool.config.min_buffer_size);
    
    let custom_config = PoolConfig::default()
        .with_num_shards(16)
        .with_tls_cache_size(8)
        .with_max_buffers_per_shard(32)
        .with_min_buffer_size(512 * 1024);
    
    let custom_pool = BufferPool::with_config(custom_config);
    println!("\nCustom pool configuration:");
    println!("  num_shards: {}", custom_pool.config.num_shards);
    println!("  tls_cache_size: {}", custom_pool.config.tls_cache_size);
    println!("  max_buffers_per_shard: {}", custom_pool.config.max_buffers_per_shard);
    println!("  min_buffer_size: {} bytes", custom_pool.config.min_buffer_size);
    
    let weird_config = PoolConfig::default().with_num_shards(100);
    println!("\nWeird config (requested 100 shards):");
    println!("  actual num_shards: {} (rounded to power-of-2, clamped to 128 max)", weird_config.num_shards);
}
