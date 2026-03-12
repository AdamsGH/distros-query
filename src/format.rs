use crate::query::{PackageInfo, PackageStatus};
use comfy_table::{
    Attribute, Cell, Color, ContentArrangement, Table,
    presets::UTF8_BORDERS_ONLY,
    Width::Fixed,
};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => Err(format!("unknown format '{other}', expected: table, json")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableLayout {
    /// One row per package entry — good for single-repo or listing mode.
    Flat,
    /// One row per repo, columns are queried package names — good for cross-distro comparison.
    Transposed,
}

impl FromStr for TableLayout {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "flat" => Ok(Self::Flat),
            "transposed" | "wide" => Ok(Self::Transposed),
            other => Err(format!("unknown layout '{other}', expected: flat, transposed")),
        }
    }
}

pub fn print_results(results: &[PackageInfo], format: OutputFormat, layout: TableLayout, color: bool) {
    match format {
        OutputFormat::Table => match layout {
            TableLayout::Flat => print_flat(results, color),
            TableLayout::Transposed => print_transposed(results, color),
        },
        OutputFormat::Json => print_json(results),
    }
}

// ── Flat table ──────────────────────────────────────────────────────────────
//
// ┌─────────┬──────────┬─────────┬──────────┬─────────┬───────────────────┐
// │ REPO    │ PACKAGE  │ VERSION │ STATUS   │ LATEST  │ MAINTAINERS       │
// ╞═════════╪══════════╪═════════╪══════════╪═════════╪═══════════════════╡
// │ openbsd │ net/curl │ 8.19.0  │ newest   │ 8.19.0  │ naddy@openbsd.org │
// └─────────┴──────────┴─────────┴──────────┴─────────┴───────────────────┘

fn print_flat(results: &[PackageInfo], color: bool) {
    if results.is_empty() {
        return;
    }

    let mut table = base_table();
    table.set_header(header_cells(
        &["REPO", "PACKAGE", "VERSION", "STATUS", "LATEST", "MAINTAINERS"],
        color,
    ));

    // Col 5 = MAINTAINERS: wrap at 40 chars so long lists don't blow the table
    table.column_mut(5)
        .unwrap()
        .set_constraint(comfy_table::ColumnConstraint::UpperBoundary(Fixed(40)));

    for r in results {
        // One maintainer per line
        let maintainers = if r.maintainers.is_empty() {
            "-".to_string()
        } else {
            r.maintainers.join("\n")
        };
        let status_label = r.status.label();

        if color {
            let (fg, attr) = status_style(&r.status);
            table.add_row(vec![
                plain_cell(&r.repo),
                plain_cell(&r.name),
                styled_cell(&r.version, fg, attr),
                styled_cell(status_label, fg, attr),
                plain_cell(&r.latest),
                plain_cell(&maintainers),
            ]);
        } else {
            table.add_row(vec![
                plain_cell(&r.repo),
                plain_cell(&r.name),
                plain_cell(&r.version),
                plain_cell(status_label),
                plain_cell(&r.latest),
                plain_cell(&maintainers),
            ]);
        }
    }

    println!("{table}");
}

// ── Transposed table ────────────────────────────────────────────────────────
//
// ┌─────────────────┬─────────────────────┬──────────────┬──────────────────┐
// │ REPO            │ python              │ pip          │ setuptools       │
// ╞═════════════════╪═════════════════════╪══════════════╪══════════════════╡
// │ arch            │ python 3.13.2       │ python-pip … │ python-setup … │
// │ debian_unstable │ python3 3.13.2      │ python3-pip  │ python3-setup …  │
// │ nixos           │ python3 3.13.2      │ -            │ -                │
// └─────────────────┴─────────────────────┴──────────────┴──────────────────┘

fn print_transposed(results: &[PackageInfo], color: bool) {
    if results.is_empty() {
        return;
    }

    // Collect repos and queried package names, preserving insertion order.
    let mut repos: Vec<String> = Vec::new();
    let mut query_pkgs: Vec<String> = Vec::new();
    let mut index: HashMap<(String, String), Vec<&PackageInfo>> = HashMap::new();

    for r in results {
        if !repos.contains(&r.repo) {
            repos.push(r.repo.clone());
        }
        if !query_pkgs.contains(&r.query_name) {
            query_pkgs.push(r.query_name.clone());
        }
        index
            .entry((r.repo.clone(), r.query_name.clone()))
            .or_default()
            .push(r);
    }

    repos.sort();

    let mut header = vec!["REPO".to_string()];
    header.extend(query_pkgs.iter().cloned());

    let mut table = base_table();
    table.set_header(header_cells(
        &header.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        color,
    ));

    // Cap each package column at 35 chars so wide results wrap instead of
    // stretching the table beyond the terminal width.
    for col_idx in 1..=query_pkgs.len() {
        if let Some(col) = table.column_mut(col_idx) {
            col.set_constraint(comfy_table::ColumnConstraint::UpperBoundary(Fixed(35)));
        }
    }

    for repo in &repos {
        let mut row: Vec<Cell> = vec![plain_cell(repo)];

        for qpkg in &query_pkgs {
            match index.get(&(repo.clone(), qpkg.clone())) {
                None => {
                    row.push(plain_cell("-"));
                }
                Some(entries) => {
                    // One "name version" per line — wraps cleanly inside the cell
                    let text = entries
                        .iter()
                        .map(|e| format!("{} {}", e.name, e.version))
                        .collect::<Vec<_>>()
                        .join("\n");

                    if color {
                        let best_status = entries
                            .iter()
                            .min_by_key(|e| status_priority(&e.status))
                            .map(|e| &e.status)
                            .unwrap();
                        let (fg, attr) = status_style(best_status);
                        row.push(styled_cell(&text, fg, attr));
                    } else {
                        row.push(plain_cell(&text));
                    }
                }
            }
        }

        table.add_row(row);
    }

    println!("{table}");
}

// ── JSON ────────────────────────────────────────────────────────────────────

fn print_json(results: &[PackageInfo]) {
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "repo":        r.repo,
                "query":       r.query_name,
                "name":        r.name,
                "version":     r.version,
                "status":      r.status.label(),
                "latest":      r.latest,
                "maintainers": r.maintainers,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&items).unwrap());
}

// ── Table helpers ────────────────────────────────────────────────────────────

fn base_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_BORDERS_ONLY)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

fn header_cells(labels: &[&str], color: bool) -> Vec<Cell> {
    labels
        .iter()
        .map(|&label| {
            let cell = Cell::new(label).add_attribute(Attribute::Bold);
            if color {
                cell.fg(Color::White)
            } else {
                cell
            }
        })
        .collect()
}

fn plain_cell(s: &str) -> Cell {
    Cell::new(s)
}

fn styled_cell(s: &str, fg: Color, attr: Attribute) -> Cell {
    Cell::new(s).fg(fg).add_attribute(attr)
}

/// Map a package status to a (foreground color, attribute) pair.
fn status_style(status: &PackageStatus) -> (Color, Attribute) {
    match status {
        PackageStatus::Newest                                    => (Color::Green,   Attribute::Bold),
        PackageStatus::Outdated                                  => (Color::Red,     Attribute::Bold),
        PackageStatus::Devel | PackageStatus::Rolling
            | PackageStatus::Unique                              => (Color::Cyan,    Attribute::NormalIntensity),
        PackageStatus::Legacy                                    => (Color::Yellow,  Attribute::NormalIntensity),
        PackageStatus::NoScheme                                  => (Color::Magenta, Attribute::NormalIntensity),
        PackageStatus::Incorrect | PackageStatus::Untrusted
            | PackageStatus::Ignored                             => (Color::Blue,    Attribute::NormalIntensity),
        PackageStatus::Unknown                                   => (Color::Green,   Attribute::NormalIntensity),
    }
}

/// Lower = better, used to pick the "best" status when a cell has multiple entries.
fn status_priority(status: &PackageStatus) -> u8 {
    match status {
        PackageStatus::Newest   => 0,
        PackageStatus::Devel    => 1,
        PackageStatus::Unique   => 2,
        PackageStatus::Rolling  => 3,
        PackageStatus::Outdated => 4,
        PackageStatus::Legacy   => 5,
        PackageStatus::NoScheme => 6,
        _                       => 7,
    }
}


