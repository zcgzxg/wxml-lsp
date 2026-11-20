//! Criterion groups: one group per query path, plus the cold first-query
//! group.
//!
//! Warm groups reuse a single server, session and project index; the cold
//! group rebuilds all of them for every iteration. The two must not be
//! compared with each other.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use wxml_lsp::bench_support::{
    backend_from_install, class_site_at, class_tokens, host_only_backend,
};

use crate::scenario::{Case, Fixture};
use crate::session::Session;

/// Registers the static tag/attribute/value hover paths.
pub(crate) fn tag(criterion: &mut Criterion, fixture: &Fixture) {
    let cases = fixture.tag_cases();
    warm(criterion, "query/tag", fixture, host_only_backend(), &cases);
}

/// Registers class extraction, class-site detection and the class LSP paths.
pub(crate) fn class(criterion: &mut Criterion, fixture: &Fixture) {
    let text = &fixture.text;
    let tokens = class_tokens(text);
    assert!(
        tokens.iter().any(|(_, _, name)| name == "card"),
        "class extract: expected a `card` token, got {tokens:?}"
    );

    criterion.bench_function("query/class/extract", |bencher| {
        bencher.iter(|| black_box(class_tokens(text)));
    });

    let mixed = fixture.class_mixed_offset();
    let site = class_site_at(text, mixed);
    assert!(
        site.as_ref().is_some_and(|(name, _, _)| name == "card"),
        "class site: expected `card` at the mixed-class offset, got {site:?}"
    );
    criterion.bench_function("query/class/site", |bencher| {
        bencher.iter(|| black_box(class_site_at(text, mixed)));
    });

    let cases = fixture.class_cases();
    warm(
        criterion,
        "query/class",
        fixture,
        host_only_backend(),
        &cases,
    );
}

/// Registers the engine-backed type hover paths.
///
/// Prints `SKIP` when the installed layout is missing; the caller only
/// registers this group after [`install_available`] succeeded.
pub(crate) fn r#type(
    criterion: &mut Criterion,
    fixture: &Fixture,
    backend: wxml_lsp::bench_support::BenchBackend,
) {
    let cases = fixture.type_cases();
    warm(criterion, "query/type", fixture, backend, &cases);
}

/// Registers the cold `first-query/*` paths.
///
/// `install` is the install root when the engine is available; the type case
/// is only registered with it.
pub(crate) fn first_query(criterion: &mut Criterion, fixture: &Fixture, install: Option<&Path>) {
    let mut group = criterion.benchmark_group("first-query");
    group.sample_size(10);

    if let Some(root) = install {
        let case = fixture.type_cases().remove(0);
        cold(
            &mut group,
            fixture,
            "type",
            || backend_from_install(root).expect("installed layout"),
            &case,
        );
    }

    let mut tags = fixture.tag_cases();
    let tag = tags.remove(0);
    cold(&mut group, fixture, "tag", host_only_backend, &tag);

    let classes = fixture.class_cases();
    let class = classes
        .into_iter()
        .find(|case| case.id == "hover")
        .expect("class hover case");
    cold(&mut group, fixture, "class", host_only_backend, &class);

    group.finish();
}

/// Runs every case once as a correctness gate, then benchmarks it warm.
fn warm(
    criterion: &mut Criterion,
    group: &str,
    fixture: &Fixture,
    backend: wxml_lsp::bench_support::BenchBackend,
    cases: &[Case],
) {
    let mut session = Session::start(backend, &fixture.root);
    session.open(&fixture.page, &fixture.text);
    for case in cases {
        case.verify(&mut session);
    }
    for case in cases {
        let id = format!("{group}/{}", case.id);
        criterion.bench_function(&id, |bencher| {
            bencher.iter(|| black_box(case.run(&mut session)));
        });
    }
    session.shutdown();
}

/// Benchmarks one cold case: fresh backend, host, server, open and query.
fn cold(
    group: &mut BenchmarkGroup<'_, WallTime>,
    fixture: &Fixture,
    name: &str,
    backend: impl Fn() -> wxml_lsp::bench_support::BenchBackend,
    case: &Case,
) {
    {
        let mut session = Session::start(backend(), &fixture.root);
        session.open(&fixture.page, &fixture.text);
        case.verify(&mut session);
        session.shutdown();
    }
    group.bench_function(name, |bencher| {
        bencher.iter(|| {
            let mut session = Session::start(backend(), &fixture.root);
            session.open(&fixture.page, &fixture.text);
            let result = case.run(&mut session);
            case.check(&result);
            session.shutdown();
            black_box(result)
        });
    });
}
