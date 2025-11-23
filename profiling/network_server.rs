/// Network Server Profiling Program
///
/// Simulates a multi-threaded network server handling concurrent connections
/// with realistic packet size distributions. This workload demonstrates:
/// - Multi-threaded contention on the pool
/// - Mixed small/large buffer allocations (64B - 64KB packets)
/// - Request/response cycles with buffer reuse
/// - TLS cache effectiveness under concurrent load
/// - Realistic Zipf distribution for packet sizes
///
/// Usage: cargo flamegraph --profile profiling --bin network_server
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use zeropool::BufferPool;

// Configuration - increased for better profiling visibility
const NUM_WORKER_THREADS: usize = 8;
const CONNECTIONS_PER_THREAD: usize = 5000; // Increased from 1000
const REQUESTS_PER_CONNECTION: usize = 100;
const SIMULATION_DURATION_SECS: u64 = 30;

// Packet size distribution (bytes)
// Based on real network traffic patterns:
// - 50% small packets (64-512 bytes): headers, ACKs, small JSON
// - 30% medium packets (1KB-8KB): API responses
// - 15% large packets (16KB-32KB): file chunks, images
// - 5% very large packets (64KB): max TCP segment size
const PACKET_SIZES: &[(usize, usize)] = &[
    (64, 50),   // 50% tiny packets
    (512, 20),  // 20% small packets
    (1024, 10), // 10% 1KB packets
    (4096, 10), // 10% 4KB packets
    (16384, 7), // 7% 16KB packets
    (32768, 2), // 2% 32KB packets
    (65536, 1), // 1% max size packets
];

fn main() {
    eprintln!("=== Network Server Profiling ===");
    eprintln!("Worker threads: {}", NUM_WORKER_THREADS);
    eprintln!("Connections per thread: {}", CONNECTIONS_PER_THREAD);
    eprintln!("Requests per connection: {}", REQUESTS_PER_CONNECTION);
    eprintln!("Target duration: {}s", SIMULATION_DURATION_SECS);
    eprintln!();

    // Create pool optimized for high concurrency
    let pool = Arc::new(
        BufferPool::builder()
            .num_shards(16)
            .tls_cache_size(6)
            .max_buffers_per_shard(32)
            .min_buffer_size(512) // Keep even small packets
            .build(),
    );

    eprintln!("Pre-allocating buffers...");
    preallocate_common_sizes(&pool);

    let start = Instant::now();
    let mut handles = Vec::new();

    eprintln!("\nStarting {} worker threads...", NUM_WORKER_THREADS);

    // Spawn worker threads
    for worker_id in 0..NUM_WORKER_THREADS {
        let pool = Arc::clone(&pool);
        let handle = thread::spawn(move || worker_thread(worker_id, pool));
        handles.push(handle);
    }

    // Wait for all workers to complete
    let mut total_requests = 0;
    let mut total_bytes = 0;

    for handle in handles {
        let (requests, bytes) = handle.join().unwrap();
        total_requests += requests;
        total_bytes += bytes;
    }

    let duration = start.elapsed();

    eprintln!("\n=== Profiling Complete ===");
    eprintln!("Duration: {:.2?}", duration);
    eprintln!("Total requests: {}", total_requests);
    eprintln!("Total data processed: {:.2} GB", total_bytes as f64 / 1e9);
    eprintln!("Throughput: {:.0} req/s", total_requests as f64 / duration.as_secs_f64());
    eprintln!("Bandwidth: {:.2} GB/s", total_bytes as f64 / duration.as_secs_f64() / 1e9);
    eprintln!("Pool stats: {} buffers in pool", pool.len());
}

/// Pre-allocate common packet sizes
fn preallocate_common_sizes(pool: &BufferPool) {
    pool.preallocate(32, 64);
    pool.preallocate(32, 512);
    pool.preallocate(32, 1024);
    pool.preallocate(16, 4096);
    pool.preallocate(8, 16384);
}

/// Worker thread handling connections
fn worker_thread(worker_id: usize, pool: Arc<BufferPool>) -> (usize, usize) {
    let mut requests_processed = 0;
    let mut bytes_processed = 0;

    if worker_id == 0 {
        eprintln!("  Worker {} started", worker_id);
    }

    // Handle multiple connections
    for conn_id in 0..CONNECTIONS_PER_THREAD {
        let (reqs, bytes) = handle_connection(&pool);
        requests_processed += reqs;
        bytes_processed += bytes;
    }

    (requests_processed, bytes_processed)
}

/// Handle a single connection with multiple requests
fn handle_connection(pool: &BufferPool) -> (usize, usize) {
    let mut requests = 0;
    let mut bytes = 0;

    for _ in 0..REQUESTS_PER_CONNECTION {
        // Receive request
        let request_size = get_packet_size();
        let mut request_buf = pool.get(request_size);
        receive_packet(&mut request_buf);
        let request_data = parse_request(&request_buf);
        bytes += request_buf.len();

        // Process request and generate response
        let response_size = get_packet_size();
        let mut response_buf = pool.get(response_size);
        generate_response(&mut response_buf, request_data);
        send_packet(&response_buf);
        bytes += response_buf.len();

        requests += 1;

        // Buffers automatically returned to pool when dropped
    }

    (requests, bytes)
}

/// Get packet size based on realistic distribution
fn get_packet_size() -> usize {
    // Simple weighted random selection
    let mut cumulative = 0;
    let random = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        % 100) as usize;

    for &(size, weight) in PACKET_SIZES {
        cumulative += weight;
        if random < cumulative {
            return size;
        }
    }

    PACKET_SIZES[0].0
}

/// Simulate receiving a network packet
fn receive_packet(buffer: &mut [u8]) {
    // Minimal work - just touch the buffer
    if !buffer.is_empty() {
        buffer[0] = 1;
    }
}

/// Simulate parsing HTTP/protocol request
fn parse_request(buffer: &[u8]) -> u32 {
    // Minimal work
    std::hint::black_box(buffer.len() as u32)
}

/// Simulate generating response data
fn generate_response(buffer: &mut [u8], request_id: u32) {
    // Minimal work
    if !buffer.is_empty() {
        buffer[0] = (request_id % 256) as u8;
    }
}

/// Simulate sending packet to network
fn send_packet(buffer: &[u8]) {
    // Minimal work
    std::hint::black_box(buffer.len());
}
