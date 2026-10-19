//! Temporary, opt-in observation for one synthetic RESET click. Never ship this module.
//! Buffer until the first post-click state request: no I/O between press and release.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use pdfcraft_engine::{FormField, FormFieldKind};
use serde_json::{Value, json};

const LIMIT: usize = 128;
static TRACE: Mutex<Option<Trace>> = Mutex::new(None);
static ENABLED: OnceLock<bool> = OnceLock::new();
static USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Trace {
    start: Instant,
    records: Vec<Value>,
    dropped: usize,
}

pub(crate) fn begin() {
    if !*ENABLED.get_or_init(|| std::env::var("PDFCRAFT_RESET_CLICK_DIAGNOSTIC").as_deref() == Ok("synthetic-reset-once"))
        || USED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }
    if let Ok(mut trace) = TRACE.lock() {
        *trace = Some(Trace { start: Instant::now(), records: Vec::with_capacity(LIMIT), dropped: 0 });
    }
}

pub(crate) fn active() -> bool {
    TRACE.lock().is_ok_and(|trace| trace.is_some())
}

pub(crate) fn synthetic(form: &[FormField]) -> bool {
    active() && form.iter().any(|f| f.name == "reset" && f.kind == FormFieldKind::PushButton)
}

pub(crate) fn record(stage: &'static str, data: Value) {
    let Ok(mut trace) = TRACE.lock() else { return };
    let Some(trace) = trace.as_mut() else { return };
    if trace.records.len() < LIMIT {
        trace.records.push(json!({"stage": stage, "instant_us": trace.start.elapsed().as_micros(), "data": data}));
    } else {
        trace.dropped = trace.dropped.saturating_add(1);
    }
}

pub(crate) fn pointer(ctx: &egui::Context, resp: &egui::Response) -> Value {
    let p = |p: Option<egui::Pos2>| p.map(|p| [p.x, p.y]);
    let mut data = ctx.input(|i| {
        json!({
            "input_time": i.time, "focused": i.focused,
            "hover": p(i.pointer.hover_pos()), "latest": p(i.pointer.latest_pos()),
            "interact": p(i.pointer.interact_pos()), "press_origin": p(i.pointer.press_origin()),
            "pressed": i.pointer.any_pressed(), "released": i.pointer.any_released(),
            "down": i.pointer.any_down(), "any_click": i.pointer.any_click(),
            "could_be_click": i.pointer.could_any_button_be_click()
        })
    });
    data["frame"] = ctx.cumulative_frame_nr().into();
    data["pass"] = ctx.cumulative_pass_nr().into();
    data["pass_index"] = ctx.current_pass_index().into();
    data["response"] = json!({"clicked": resp.clicked(), "hovered": resp.hovered(),
        "contains_pointer": resp.contains_pointer(), "down_on": resp.is_pointer_button_down_on(),
        "dragged": resp.dragged(), "enabled": resp.enabled()});
    data
}

pub(crate) fn raw(ctx: &egui::Context, input: &egui::RawInput, existing_count: usize, passes: u64) {
    if !active() {
        return;
    }
    let events: Vec<Value> = input
        .events
        .iter()
        .take(16)
        .enumerate()
        .map(|(n, event)| {
            let mut item = match event {
                egui::Event::PointerMoved(p) => json!({"kind": "move", "position": [p.x, p.y]}),
                egui::Event::PointerButton { pos, button, pressed, .. } => {
                    json!({"kind": "button", "position": [pos.x, pos.y], "button": format!("{button:?}"), "pressed": pressed})
                }
                egui::Event::PointerGone => json!({"kind": "gone"}),
                egui::Event::WindowFocused(focused) => json!({"kind": "focus", "focused": focused}),
                _ => json!({"kind": "other"}),
            };
            item["origin"] = if n < existing_count { "existing" } else { "injected" }.into();
            item
        })
        .collect();
    record(
        "raw_input",
        json!({"time": input.time, "focused": input.focused,
        "frame": ctx.cumulative_frame_nr(), "pass": ctx.cumulative_pass_nr(),
        "pass_index": ctx.current_pass_index(), "control_passes": passes,
        "events": events, "omitted_events": input.events.len().saturating_sub(16)}),
    );
}

pub(crate) fn finish() {
    let Ok(mut slot) = TRACE.lock() else { return };
    let Some(trace) = slot.take() else { return };
    drop(slot);
    // Diagnostic output is best-effort and contains no field values, paths, text or control token.
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    for row in trace.records {
        let _ = writeln!(stderr, "RESET_CLICK_DIAGNOSTIC {row}");
    }
    let _ = writeln!(stderr, "RESET_CLICK_DIAGNOSTIC {{\"stage\":\"end\",\"dropped\":{}}}", trace.dropped);
}
