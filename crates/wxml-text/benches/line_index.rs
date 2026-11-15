//! Benchmarks for [`wxml_text::line_index::LineIndex`].

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use wxml_text::line_index::{LineIndex, Position};

/// Builds a synthetic template-like document with ASCII and CJK content.
fn document(lines: usize) -> String {
    let mut text = String::new();
    for line in 0..lines {
        text.push_str("<view class=\"row\">{{ user.Name }} 中文内容 ");
        text.push_str(&line.to_string());
        text.push_str("</view>\n");
    }
    text
}

fn bench_build(c: &mut Criterion) {
    let text = document(2_000);
    c.bench_function("line_index/build", |b| {
        b.iter(|| LineIndex::new(black_box(&text)));
    });
}

fn bench_position(c: &mut Criterion) {
    let text = document(2_000);
    let index = LineIndex::new(&text);
    let offsets: Vec<usize> = (0..=text.len())
        .step_by(97)
        .filter(|&offset| text.is_char_boundary(offset))
        .collect();
    c.bench_function("line_index/position", |b| {
        b.iter(|| {
            for &offset in &offsets {
                black_box(index.position(black_box(&text), black_box(offset)));
            }
        });
    });
}

fn bench_offset(c: &mut Criterion) {
    let text = document(2_000);
    let index = LineIndex::new(&text);
    let positions: Vec<Position> = (0..2_000).map(|line| Position::new(line, 10)).collect();
    c.bench_function("line_index/offset", |b| {
        b.iter(|| {
            for &position in &positions {
                black_box(index.offset(black_box(&text), black_box(position)));
            }
        });
    });
}

criterion_group!(benches, bench_build, bench_position, bench_offset);
criterion_main!(benches);
