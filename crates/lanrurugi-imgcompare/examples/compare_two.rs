//! Ad-hoc manual verification against two real archives — paths come from env vars
//! (`LANRURUGI_TEST_COMPARE_A`/`_B`), never hardcoded, per this repo's own rule against
//! committing real work titles/paths to source. Not a `#[test]` (no CI value — this is for
//! manually eyeballing real output during development, see `cargo run --example compare_two`).

fn main() {
    let a = std::env::var("LANRURUGI_TEST_COMPARE_A").expect("set LANRURUGI_TEST_COMPARE_A");
    let b = std::env::var("LANRURUGI_TEST_COMPARE_B").expect("set LANRURUGI_TEST_COMPARE_B");

    let a_size = std::fs::metadata(&a).expect("stat a").len();
    let b_size = std::fs::metadata(&b).expect("stat b").len();

    let start = std::time::Instant::now();
    let result = lanrurugi_imgcompare::compare_archives(
        std::path::Path::new(&a),
        std::path::Path::new(&b),
        a.clone(),
        a_size,
        b.clone(),
        b_size,
    )
    .expect("comparison failed");
    let elapsed = start.elapsed();

    println!("a_total_pages: {}", result.a_total_pages);
    println!("b_total_pages: {}", result.b_total_pages);
    println!("aligned_pairs: {}", result.aligned_pairs);
    println!(
        "likely_different_language: {}",
        result.likely_different_language
    );
    println!("recommendation: {:?}", result.recommendation);
    println!("elapsed: {elapsed:?}");
    println!();
    println!("samples:");
    for s in &result.samples {
        println!(
            "  a[{}] sharpness={:.1}  vs  b[{}] sharpness={:.1}  ({})",
            s.a_page_index,
            s.a_sharpness,
            s.b_page_index,
            s.b_sharpness,
            if s.a_sharpness > s.b_sharpness {
                "A sharper"
            } else {
                "B sharper"
            }
        );
    }
}
