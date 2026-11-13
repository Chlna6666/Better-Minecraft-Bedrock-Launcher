//! Run the same corpus with --no-default-features, then --features simd.
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

fn main() {
    println!(
        "simd={}, parallel={}",
        cfg!(feature = "simd"),
        cfg!(feature = "parallel")
    );
    for len in [8, 128, 16_384] {
        let input = format!(
            "{{Text:\"{}\",Seed:-9223372036854775808L}}",
            "x".repeat(len)
        );
        measure(&format!("string_bytes={len}"), &input);
    }
    let fields: Vec<_> = (0..128)
        .map(|index| format!("key{index}:\"short\""))
        .collect();
    measure("short_fields=128", &format!("{{{}}}", fields.join(",")));
}

fn measure(label: &str, input: &str) {
    for _ in 0..1_000 {
        black_box(bedrock_nbt::snbt::parse(black_box(input)).unwrap());
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let mut iterations = 0;
        while start.elapsed() < Duration::from_millis(200) {
            for _ in 0..100 {
                black_box(bedrock_nbt::snbt::parse(black_box(input)).unwrap());
            }
            iterations += 100;
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{label}, median_ns={:.1}, min_ns={:.1}, max_ns={:.1}",
        samples[3], samples[0], samples[6]
    );
}
