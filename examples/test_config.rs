//! Example demonstrating ZeroPool configuration options
use zeropool::ZeroPool;

fn main() {
    let pool = ZeroPool::new();
    println!("Created pool with default configuration");
    println!("Pool has {} buffers", pool.len());

    let custom_pool = ZeroPool::new()
        .tls_cache_size(8)
        .max_buffers_per_class(32)
        .min_buffer_size(4096)
        .batch_size(4);

    println!("\nCreated custom pool");
    println!("Pool has {} buffers", custom_pool.len());

    {
        let buf = custom_pool.alloc(1024 * 1024);
        println!("\nAllocated buffer of size {} bytes", buf.len());
    }
    println!("Deallocated buffer back to pool");
    println!("Pool now has {} buffers", custom_pool.len());
}
