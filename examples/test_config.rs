use zeropool::BufferPool;

fn main() {
    // Simple default pool
    let pool = BufferPool::new();
    println!("Created pool with default configuration");
    println!("Pool has {} buffers", pool.len());

    // Custom configuration with builder
    let custom_pool = BufferPool::builder()
        .num_shards(16)
        .tls_cache_size(8)
        .max_buffers_per_shard(32)
        .min_buffer_size(512 * 1024)
        .build();

    println!("\nCreated custom pool with builder");
    println!("Pool has {} buffers", custom_pool.len());

    // Test buffer operations
    {
        let buf = custom_pool.get(1024 * 1024);
        println!("\nGot buffer of size {} bytes", buf.len());
        // Buffer automatically returned to pool when dropped
    }
    println!("Returned buffer to pool");
    println!("Pool now has {} buffers", custom_pool.len());
}
