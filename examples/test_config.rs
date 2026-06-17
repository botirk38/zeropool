//! Example demonstrating BufferPool configuration options
use zeropool::BufferPool;

/// Main function demonstrating pool configuration and usage
fn main() {
    // Simple default pool
    let pool = BufferPool::new();
    println!("Created pool with default configuration");
    println!("Pool has {} buffers", pool.len());

    // Custom configuration
    let custom_pool = BufferPool::new()
        .tls_cache_size(8)
        .max_buffers_per_class(32)
        .min_buffer_size(4096)
        .batch_size(4);

    println!("\nCreated custom pool");
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
