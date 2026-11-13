//! Release benchmark of the terminal update and status codec: the server's
//! update construction and encoding, and the client's decoding and apply,
//! for complete snapshots and deltas at several grid sizes. Run it through
//! `mise run bench:wire`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use huterm_protocol::{
    Cell, CellColor, CellStyle, CellText, GridSize, RowPrompt, TerminalRow,
    TerminalSnapshot,
};

use crate::caps::Direction;
use crate::message::{Message, decode_message, encode_message};
use crate::update::{
    TerminalUpdate, UpdateEncoder, ViewMetadata, ViewerView, view_digest,
};
use crate::vectors::{context, sample_snapshot, sample_status, scope, viewer};
use crate::viewer::ViewerStatus;

const GRIDS: [(u16, u16); 3] = [(80, 24), (200, 60), (400, 120)];

/// How a fixture's rows look.
#[derive(Clone, Copy)]
enum Content {
    /// Full rows of ASCII text in one style.
    Plain,
    /// Full rows of ASCII text with a style change every 8 columns:
    /// alternating indexed foregrounds, every other group bold.
    Styled,
    /// Shell-like rows: a short line of text, then cells with no text.
    Sparse,
}

impl Content {
    const ALL: [Self; 3] = [Self::Plain, Self::Styled, Self::Sparse];

    const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Styled => "styled",
            Self::Sparse => "sparse",
        }
    }

    /// Row `index`'s cells, varied by `salt` so a changed row differs.
    fn row(self, columns: u16, index: usize, salt: usize) -> Arc<TerminalRow> {
        let columns = usize::from(columns);
        let text_columns = match self {
            Self::Plain | Self::Styled => columns,
            Self::Sparse => (index * 7 + salt) % (columns / 2) + 1,
        };
        let cells = (0..columns)
            .map(|column| {
                let text = if column < text_columns {
                    let byte = b'!'
                        + u8::try_from((index * 31 + column + salt) % 94)
                            .unwrap();
                    CellText::from(char::from(byte))
                } else {
                    CellText::EMPTY
                };
                let group = column / 8;
                let (foreground, style) = match self {
                    Self::Styled => (
                        CellColor::Indexed(u8::try_from(group % 16).unwrap()),
                        CellStyle {
                            bold: group % 2 == 1,
                            ..CellStyle::default()
                        },
                    ),
                    Self::Plain | Self::Sparse => {
                        (CellColor::DefaultForeground, CellStyle::default())
                    }
                };
                Cell {
                    text,
                    foreground,
                    background: CellColor::DefaultBackground,
                    style,
                }
            })
            .collect();
        Arc::new(TerminalRow {
            cells,
            wrapped: false,
            wrap_continuation: false,
            prompt: RowPrompt::None,
        })
    }
}

fn snapshot(
    metadata: ViewMetadata,
    rows: Vec<Arc<TerminalRow>>,
) -> Arc<TerminalSnapshot> {
    Arc::new(metadata.snapshot(sample_snapshot().terminal_id, rows))
}

fn metadata(size: GridSize) -> ViewMetadata {
    let mut metadata = ViewMetadata::of(&sample_snapshot());
    metadata.size = size;
    metadata.cursor = None;
    metadata
}

/// Elapsed nanoseconds of each sample, sorted.
struct Samples(Vec<u128>);

impl Samples {
    fn p50(&self) -> u128 {
        self.0[self.0.len() / 2]
    }

    fn p95(&self) -> u128 {
        self.0[self.0.len() * 95 / 100]
    }
}

fn sorted(mut samples: Vec<u128>) -> Samples {
    samples.sort_unstable();
    Samples(samples)
}

fn encode(update: TerminalUpdate) -> Vec<u8> {
    encode_message(&Message::TerminalUpdate(update), scope(), context(0).caps)
        .unwrap()
}

fn decode(bytes: &[u8]) -> TerminalUpdate {
    match decode_message(bytes, Direction::ServerToClient, context(0)) {
        Ok(Message::TerminalUpdate(update)) => update,
        other => panic!("not an update: {other:?}"),
    }
}

/// Bytes per cell with two decimals.
fn per_cell(bytes: usize, size: GridSize) -> String {
    let hundredths = bytes * 100 / size.cells();
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Enough samples for a stable median that keep the largest grid's run
/// short.
fn iterations(cells: usize) -> usize {
    (2_000_000 / cells).clamp(50, 1000)
}

/// One measured operation's report and the budgets it exceeded.
struct Measured {
    line: String,
    failures: Vec<String>,
}

/// The most one operation may take at the largest grid: the server's
/// build and encode, and the client's decode and apply, each as a median,
/// and the update's bytes.
struct Budget {
    fixture: &'static str,
    op: &'static str,
    server_us: u128,
    client_us: u128,
    bytes: usize,
}

/// The largest grid, where budgets apply.
const GATED_GRID: (u16, u16) = (400, 120);

// PROVISIONAL: Apple silicon measurements times about three, until CI
// measurements replace them.
const BUDGETS: &[Budget] = &[
    Budget {
        fixture: "plain",
        op: "complete",
        server_us: 2_000,
        client_us: 1_300,
        bytes: 49_242,
    },
    Budget {
        fixture: "styled",
        op: "complete",
        server_us: 2_600,
        client_us: 1_800,
        bytes: 60_958,
    },
    Budget {
        fixture: "sparse",
        op: "complete",
        server_us: 2_100,
        client_us: 1_300,
        bytes: 49_242,
    },
    Budget {
        fixture: "plain",
        op: "delta-row",
        server_us: 30,
        client_us: 15,
        bytes: 682,
    },
    Budget {
        fixture: "styled",
        op: "delta-row",
        server_us: 40,
        client_us: 20,
        bytes: 855,
    },
    Budget {
        fixture: "sparse",
        op: "delta-row",
        server_us: 30,
        client_us: 15,
        bytes: 682,
    },
];

/// The budgets an operation's medians and bytes exceed, if it has any.
fn over_budget(
    content: Content,
    size: GridSize,
    op: &str,
    (server_ns, client_ns, bytes): (u128, u128, usize),
) -> Vec<String> {
    let gated = (size.columns, size.rows) == GATED_GRID;
    let Some(budget) = BUDGETS.iter().find(|budget| {
        gated && budget.fixture == content.name() && budget.op == op
    }) else {
        return Vec::new();
    };
    let name =
        format!("{} {op} at {}x{}", budget.fixture, size.columns, size.rows);
    let mut failures = Vec::new();
    if server_ns > budget.server_us * 1000 {
        failures.push(format!(
            "{name}: server median {server_ns} ns over {} us",
            budget.server_us
        ));
    }
    if client_ns > budget.client_us * 1000 {
        failures.push(format!(
            "{name}: client median {client_ns} ns over {} us",
            budget.client_us
        ));
    }
    if bytes > budget.bytes {
        failures.push(format!("{name}: {bytes} bytes over {}", budget.bytes));
    }
    failures
}

fn complete(content: Content, size: GridSize) -> Measured {
    let rows: Vec<_> = (0..usize::from(size.rows))
        .map(|index| content.row(size.columns, index, 0))
        .collect();
    let current = snapshot(metadata(size), rows);
    let mut encoder = UpdateEncoder::new(viewer(), 0);
    let mut view = ViewerView::new(0);
    let (mut build, mut digest, mut encoding, mut decoding, mut apply) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut bytes = 0;
    for _ in 0..iterations(size.cells()) {
        encoder.resync();
        let started = Instant::now();
        let update = encoder.next(black_box(&current), false).unwrap();
        build.push(started.elapsed().as_nanos());
        let started = Instant::now();
        black_box(view_digest(&update.metadata, black_box(&current.rows), 0));
        digest.push(started.elapsed().as_nanos());
        let started = Instant::now();
        let message = encode(update);
        encoding.push(started.elapsed().as_nanos());
        bytes = message.len();
        let started = Instant::now();
        let update = decode(black_box(&message));
        decoding.push(started.elapsed().as_nanos());
        let started = Instant::now();
        let disposal = view.apply(&update).unwrap();
        apply.push(started.elapsed().as_nanos());
        assert_eq!(disposal.outcome, crate::update::UpdateOutcome::Applied);
    }
    let shown = view.snapshot(current.terminal_id).unwrap();
    assert_eq!(shown.rows, current.rows, "the client shows the snapshot");
    let (build, digest, encoding, decoding, apply) = (
        sorted(build),
        sorted(digest),
        sorted(encoding),
        sorted(decoding),
        sorted(apply),
    );
    let server = build.p50() + encoding.p50();
    let client = decoding.p50() + apply.p50();
    Measured {
        line: format!(
            "fixture={} grid={}x{} op=complete bytes={bytes} bytes_per_cell={} \
             build_ns_p50={} digest_ns_p50={} encode_ns_p50={} decode_ns_p50={} \
             apply_ns_p50={} server_ns_p50={server} client_ns_p50={client} \
             server_ns_p95={} client_ns_p95={}",
            content.name(),
            size.columns,
            size.rows,
            per_cell(bytes, size),
            build.p50(),
            digest.p50(),
            encoding.p50(),
            decoding.p50(),
            apply.p50(),
            build.p95() + encoding.p95(),
            decoding.p95() + apply.p95(),
        ),
        failures: over_budget(
            content,
            size,
            "complete",
            (server, client, bytes),
        ),
    }
}

/// How each delta of a fixture changes the rows.
#[derive(Clone, Copy)]
enum Change {
    /// One row changes.
    Row,
    /// The rows scroll by one line, which brings in one new row.
    Scroll,
    /// Every row changes.
    All,
}

impl Change {
    const ALL: [Self; 3] = [Self::Row, Self::Scroll, Self::All];

    const fn name(self) -> &'static str {
        match self {
            Self::Row => "delta-row",
            Self::Scroll => "delta-scroll",
            Self::All => "delta-all",
        }
    }
}

fn delta(content: Content, size: GridSize, change: Change) -> Measured {
    let count = usize::from(size.rows);
    // Two snapshots the deltas alternate between, each a delta of the
    // stated change against the other.
    let lines: Vec<_> = (0..=count)
        .map(|index| content.row(size.columns, index, 0))
        .collect();
    let first: Vec<_> = lines[..count].to_vec();
    let second: Vec<_> = match change {
        Change::Row => {
            let mut rows = first.clone();
            rows[count / 2] = content.row(size.columns, count / 2, 1);
            rows
        }
        Change::Scroll => lines[1..].to_vec(),
        Change::All => (0..count)
            .map(|index| content.row(size.columns, index, 1))
            .collect(),
    };
    let snapshots = [
        snapshot(metadata(size), first),
        snapshot(metadata(size), second),
    ];
    let mut encoder = UpdateEncoder::new(viewer(), 0);
    let mut view = ViewerView::new(0);
    view.apply(&decode(&encode(
        encoder.next(&snapshots[1], false).unwrap(),
    )))
    .unwrap();
    let (mut server, mut client) = (Vec::new(), Vec::new());
    let mut bytes = Vec::new();
    for iteration in 0..iterations(size.cells()) {
        let current = &snapshots[iteration % 2];
        let started = Instant::now();
        let message = encode(encoder.next(black_box(current), false).unwrap());
        server.push(started.elapsed().as_nanos());
        bytes.push(message.len());
        let started = Instant::now();
        let disposal = view.apply(&decode(black_box(&message))).unwrap();
        client.push(started.elapsed().as_nanos());
        assert_eq!(disposal.outcome, crate::update::UpdateOutcome::Applied);
    }
    let shown = view.snapshot(snapshots[0].terminal_id).unwrap();
    let last = &snapshots[(iterations(size.cells()) - 1) % 2];
    assert_eq!(shown.rows, last.rows, "the client shows the last snapshot");
    bytes.sort_unstable();
    let bytes = bytes[bytes.len() / 2];
    let (server, client) = (sorted(server), sorted(client));
    Measured {
        line: format!(
            "fixture={} grid={}x{} op={} bytes_p50={} server_ns_p50={} \
             client_ns_p50={} server_ns_p95={} client_ns_p95={}",
            content.name(),
            size.columns,
            size.rows,
            change.name(),
            bytes,
            server.p50(),
            client.p50(),
            server.p95(),
            client.p95(),
        ),
        failures: over_budget(
            content,
            size,
            change.name(),
            (server.p50(), client.p50(), bytes),
        ),
    }
}

fn status() -> Measured {
    let message = Message::ViewerStatus(ViewerStatus {
        viewer: viewer(),
        status: sample_status(),
    });
    let (mut encoding, mut decoding) = (Vec::new(), Vec::new());
    let mut bytes = 0;
    for _ in 0..1000 {
        let started = Instant::now();
        let encoded =
            encode_message(black_box(&message), scope(), context(0).caps)
                .unwrap();
        encoding.push(started.elapsed().as_nanos());
        bytes = encoded.len();
        let started = Instant::now();
        let decoded = decode_message(
            black_box(&encoded),
            Direction::ServerToClient,
            context(0),
        )
        .unwrap();
        decoding.push(started.elapsed().as_nanos());
        assert_eq!(decoded, message);
    }
    let (encoding, decoding) = (sorted(encoding), sorted(decoding));
    Measured {
        line: format!(
            "fixture=status bytes={bytes} encode_ns_p50={} decode_ns_p50={}",
            encoding.p50(),
            decoding.p50()
        ),
        failures: Vec::new(),
    }
}

#[test]
#[ignore = "release benchmark; run through mise run bench:wire"]
fn wire_benchmark() {
    let mut failures = Vec::new();
    let mut report = |measured: Measured| {
        println!("wire {}", measured.line);
        failures.extend(measured.failures);
    };
    for (columns, rows) in GRIDS {
        let size = GridSize { columns, rows };
        for content in Content::ALL {
            report(complete(content, size));
            for change in Change::ALL {
                report(delta(content, size, change));
            }
        }
    }
    report(status());
    assert!(
        failures.is_empty(),
        "budgets exceeded:\n{}",
        failures.join("\n")
    );
}
