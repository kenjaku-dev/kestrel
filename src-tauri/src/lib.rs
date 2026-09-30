// Phase 1 Tauri backend: IPC commands + DTO layer + shared state over the
// `kestrel-fs` engine. See `state.rs` (the `!Sync` crux), `dto.rs` (wire
// types) and `commands.rs` (the Phase 1 command set) for the contract notes.
pub mod commands;
pub mod dto;
pub mod state;

use tauri::Manager;

/// Dev-only fixture override for the 10k perf gate (MIGRATION.md §6).
///
/// Query strings cannot be passed to the bundled Tauri window, so the
/// fixture path (`ui/src/main.ts`, `?rows=N&perf=1`) is unreachable in the
/// real binary — exactly the gap the perf lane exists to close. These flags
/// append the matching query to the loaded URL once, before first paint:
///
/// ```sh
/// kestrel-tauri --rows 10000 --perf [--path /some/dir] [--open file.txt]
///               [--autoscroll]
/// ```
///
/// `--open <name>` appends `&open=<name>`; the UI auto-activates that row
/// after its first listing — the exact Enter/double-click code path — so
/// `open_path` is exercisable without synthetic input. No flags → the
/// startup URL is untouched. The override lives here (not in the UI) so
/// production UI code has no test-only branches beyond reading the query.
fn fixture_query() -> Option<String> {
    let mut args = std::env::args().skip(1);
    let mut rows: Option<String> = None;
    let mut perf = false;
    let mut path: Option<String> = None;
    let mut open: Option<String> = None;
    let mut autoscroll = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rows" => rows = args.next(),
            "--perf" => perf = true,
            "--path" => path = args.next(),
            "--open" => open = args.next(),
            "--autoscroll" => autoscroll = true,
            _ => {}
        }
    }
    if rows.is_none() && !perf && path.is_none() && open.is_none() && !autoscroll {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = rows {
        parts.push(format!("rows={n}"));
    }
    if perf {
        parts.push("perf=1".to_string());
    }
    if let Some(p) = path {
        parts.push(format!("path={}", encode_query_value(&p)));
    }
    if let Some(o) = open {
        parts.push(format!("open={}", encode_query_value(&o)));
    }
    if autoscroll {
        parts.push("autoscroll=1".to_string());
    }
    Some(parts.join("&"))
}

/// Minimal query-value encoding for dev flags: ASCII paths only need spaces
/// escaped (`Url::set_query` takes the raw string and a bare space would
/// corrupt navigation); everything else passes through to
/// `URLSearchParams`, which decodes the rest.
fn encode_query_value(raw: &str) -> String {
    raw.replace(' ', "%20")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let fixture = fixture_query();
    tauri::Builder::default()
        .setup(|app| {
            let backend =
                state::Backend::try_new().map_err(|e| std::io::Error::other(e.to_string()))?;
            app.manage(backend);
            Ok(())
        })
        .on_page_load(move |window, payload| {
            let Some(query) = fixture.as_deref() else {
                return;
            };
            // Navigate exactly once: after the redirect the URL already
            // carries a query and this becomes a no-op.
            if payload.url().query().is_some() {
                return;
            }
            let mut url = payload.url().clone();
            url.set_query(Some(query));
            let _ = window.navigate(url);
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_start,
            commands::scan_cancel,
            commands::stat,
            commands::open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
