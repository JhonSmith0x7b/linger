//! Small transparent desktop surface. Only painted controls intercept pointer input.
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{Emitter, Manager};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Orb,
    Notice,
    Tools,
    Chat,
    Translate,
    More,
    Bubble,
}
impl Layout {
    fn size(self) -> (f64, f64) {
        match self {
            Self::Orb => (96.0, 56.0),
            Self::Notice => (192.0, 88.0),
            Self::Tools => (320.0, 320.0),
            Self::Bubble => (320.0, 132.0),
            Self::Chat | Self::Translate | Self::More => (320.0, 440.0),
        }
    }
}
#[derive(Default)]
pub struct Overlay {
    layout: Mutex<Layout>,
    interacting: AtomicBool,
    ignored: AtomicBool,
    hit_regions: Mutex<Option<HitRegions>>,
    placement: Mutex<Option<Placement>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Placement {
    width: f64,
    height: f64,
    orb_x: f64,
    orb_y: f64,
    panel_x: f64,
    panel_y: f64,
    panel_width: f64,
    panel_height: f64,
    controls_left: bool,
}

// Preserve the orb's desktop center. Only the surrounding surface moves/resizes.
fn placement(
    mode: Layout,
    anchor: (f64, f64),
    scale: f64,
    work: [f64; 4],
) -> ((f64, f64), Placement) {
    let (left, top, right, bottom) = (work[0], work[1], work[0] + work[2], work[1] + work[3]);
    let row_width = if mode == Layout::Tools { 280.0 } else { 96.0 };
    let controls_left = anchor.0 - left >= (row_width - 28.0) * scale;
    let row_left = anchor.0
        - if controls_left {
            row_width - 28.0
        } else {
            28.0
        } * scale;
    let row_top = anchor.1 - 28.0 * scale;
    let panel_height = if mode == Layout::Orb {
        0.0
    } else {
        (mode.size().1 - 64.0).min((work[3] / scale - 64.0).max(1.0))
    };
    let panel_width = (if mode == Layout::Notice {
        192.0_f64
    } else {
        320.0_f64
    })
    .min(work[2] / scale);
    let panel_left = (anchor.0 - (panel_width - 28.0) * scale)
        .clamp(left, (right - panel_width * scale).max(left));
    let above = anchor.1 - top >= (panel_height + 36.0) * scale;
    let panel_top = if above {
        anchor.1 - (panel_height + 36.0) * scale
    } else {
        anchor.1 + 36.0 * scale
    };
    // If neither side fits (small monitor), reduce panel height to available space.
    let panel_height = if above {
        panel_height
    } else {
        panel_height.min(((bottom - panel_top) / scale).max(1.0))
    };
    let (x, y, width, height) = if mode == Layout::Orb {
        (row_left, row_top, row_width, 56.0)
    } else {
        let x = row_left.min(panel_left);
        let y = row_top.min(panel_top);
        (
            x,
            y,
            ((row_left + row_width * scale).max(panel_left + panel_width * scale) - x) / scale,
            ((row_top + 56.0 * scale).max(panel_top + panel_height * scale) - y) / scale,
        )
    };
    (
        (x, y),
        Placement {
            width,
            height,
            orb_x: (anchor.0 - x) / scale,
            orb_y: (anchor.1 - y) / scale,
            panel_x: (panel_left - x) / scale,
            panel_y: (panel_top - y) / scale,
            panel_width,
            panel_height,
            controls_left,
        },
    )
}

// WebView CSS pixels need not equal native logical pixels (Windows zoom/DPI).
#[derive(Clone, Debug, Deserialize)]
pub struct HitRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
    orb: bool,
}
#[derive(Debug, Deserialize)]
pub struct HitRegions {
    width: f64,
    height: f64,
    regions: Vec<HitRegion>,
}
impl HitRegions {
    fn hit(&self, x: f64, y: f64, width: f64, height: f64) -> (bool, bool) {
        let x = x * self.width / width;
        let y = y * self.height / height;
        let mut orb = false;
        let mut inside = false;
        for region in &self.regions {
            let matched = rounded_rect(
                x,
                y,
                region.x,
                region.y,
                region.x + region.width,
                region.y + region.height,
                region.radius,
            );
            inside |= matched;
            orb |= matched && region.orb;
        }
        (orb, inside)
    }
}
#[tauri::command]
pub fn compact_set_hit_regions(app: tauri::AppHandle, regions: HitRegions) -> Result<(), String> {
    if !regions.width.is_finite()
        || !regions.height.is_finite()
        || regions.width <= 0.0
        || regions.height <= 0.0
        || regions.regions.len() > 12
        || regions.regions.iter().any(|r| {
            ![r.x, r.y, r.width, r.height, r.radius]
                .iter()
                .all(|v| v.is_finite())
                || r.width <= 0.0
                || r.height <= 0.0
                || r.radius < 0.0
                || r.radius > r.width.min(r.height) / 2.0
        })
    {
        return Err("无效的悬浮界面区域".into());
    }
    *app.state::<Overlay>().hit_regions.lock().unwrap() = Some(regions);
    Ok(())
}

fn circle(x: f64, y: f64, cx: f64, cy: f64, radius: f64) -> bool {
    (x - cx).powi(2) + (y - cy).powi(2) <= radius.powi(2)
}
fn rounded_rect(x: f64, y: f64, left: f64, top: f64, right: f64, bottom: f64, radius: f64) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    circle(x, y, cx, cy, radius)
}
fn hit(layout: Layout, width: f64, height: f64, x: f64, y: f64, speaking: bool) -> bool {
    if circle(x, y, width - 28.0, height - 28.0, 24.0) {
        return true;
    }
    if circle(x, y, width - 76.0, height - 28.0, 16.0) {
        return true;
    }
    match layout {
        Layout::Orb | Layout::Notice => false,
        Layout::Tools => (0..if speaking { 4 } else { 3 })
            .any(|i| circle(x, y, width - 118.0 - 42.0 * i as f64, height - 28.0, 18.0)),
        _ => rounded_rect(x, y, 4.0, 4.0, width - 4.0, height - 64.0, 16.0),
    }
}

const POSITION_KEY: &str = "compact_position_v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedPosition {
    monitor: Option<String>,
    x: f64,
    y: f64,
}
impl SavedPosition {
    fn anchor(&self, area: [f64; 4], scale: f64) -> Option<(f64, f64)> {
        if !self.x.is_finite() || !self.y.is_finite() || area[2] <= 0.0 || area[3] <= 0.0 {
            return None;
        }
        let margin_x = (28.0 * scale).min(area[2] / 2.0);
        let margin_y = (28.0 * scale).min(area[3] / 2.0);
        Some((
            (area[0] + self.x * area[2]).clamp(area[0] + margin_x, area[0] + area[2] - margin_x),
            (area[1] + self.y * area[3]).clamp(area[1] + margin_y, area[1] + area[3] - margin_y),
        ))
    }
}
fn work_area(monitor: &tauri::Monitor) -> [f64; 4] {
    let area = monitor.work_area();
    [
        area.position.x as f64,
        area.position.y as f64,
        area.size.width as f64,
        area.size.height as f64,
    ]
}

// Poll the ball center, not the window origin: expanding cards must not move the saved ball.
fn remember_position(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
) -> Result<String, String> {
    let state = app.state::<Overlay>();
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let pos = window.inner_position().map_err(|e| e.to_string())?;
    let Some(p) = *state.placement.lock().unwrap() else {
        return Err("悬浮球尚未定位".into());
    };
    let anchor = (
        pos.x as f64 + p.orb_x * scale,
        pos.y as f64 + p.orb_y * scale,
    );
    let monitors = app.available_monitors().map_err(|e| e.to_string())?;
    let monitor = monitors
        .iter()
        .find(|monitor| {
            let a = work_area(monitor);
            anchor.0 >= a[0]
                && anchor.0 <= a[0] + a[2]
                && anchor.1 >= a[1]
                && anchor.1 <= a[1] + a[3]
        })
        .ok_or("悬浮球不在可见屏幕内")?;
    let area = work_area(monitor);
    serde_json::to_string(&SavedPosition {
        monitor: monitor.name().cloned(),
        x: ((anchor.0 - area[0]) / area[2] * 100000.0).round() / 100000.0,
        y: ((anchor.1 - area[1]) / area[3] * 100000.0).round() / 100000.0,
    })
    .map_err(|e| e.to_string())
}

pub fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("compact") {
        return window.show().map_err(|e| e.to_string());
    }
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        "compact",
        tauri::WebviewUrl::App("index.html?compact=true".into()),
    )
    .title("Linger · 陪伴球")
    .inner_size(96.0, 56.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .focused(false)
    .visible(false)
    .accept_first_mouse(true)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true);
    if let Some(monitor) = app.primary_monitor().map_err(|e| e.to_string())? {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        builder = builder.position(
            (area.position.x as f64 + area.size.width as f64) / scale - 120.0,
            (area.position.y as f64 + area.size.height as f64) / scale - 110.0,
        );
    }
    let window = builder.build().map_err(|e| e.to_string())?;
    let core = app.state::<std::sync::Arc<crate::Core>>();
    let saved = core
        .store
        .get_meta(POSITION_KEY)?
        .and_then(|value| serde_json::from_str::<SavedPosition>(&value).ok());
    if let Some(saved) = saved {
        let monitors = app.available_monitors().map_err(|e| e.to_string())?;
        let monitor = monitors
            .iter()
            .find(|m| saved.monitor.is_some() && m.name() == saved.monitor.as_ref())
            .cloned()
            .or(app.primary_monitor().map_err(|e| e.to_string())?)
            .or_else(|| monitors.first().cloned());
        if let Some(monitor) = monitor {
            let scale = monitor.scale_factor();
            let area = work_area(&monitor);
            if let Some(anchor) = saved.anchor(area, scale) {
                let ((x, y), p) = placement(Layout::Orb, anchor, scale, area);
                window
                    .set_position(tauri::PhysicalPosition::new(
                        x.round() as i32,
                        y.round() as i32,
                    ))
                    .map_err(|e| e.to_string())?;
                window
                    .set_size(tauri::PhysicalSize::new(
                        (p.width * scale).round() as u32,
                        (p.height * scale).round() as u32,
                    ))
                    .map_err(|e| e.to_string())?;
                *app.state::<Overlay>().placement.lock().unwrap() = Some(p);
            }
        }
    }
    window.show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn compact_set_layout(app: tauri::AppHandle, mode: Layout) -> Result<Placement, String> {
    let window = app.get_webview_window("compact").ok_or("陪伴球尚未打开")?;
    let state = app.state::<Overlay>();
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let old_size = window.inner_size().map_err(|e| e.to_string())?;
    let old_pos = window.inner_position().map_err(|e| e.to_string())?;
    let previous = *state.placement.lock().unwrap();
    let anchor = previous
        .map(|p| {
            (
                old_pos.x as f64 + p.orb_x * scale,
                old_pos.y as f64 + p.orb_y * scale,
            )
        })
        .unwrap_or((
            old_pos.x as f64 + old_size.width as f64 - 28.0 * scale,
            old_pos.y as f64 + old_size.height as f64 - 28.0 * scale,
        ));
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("无法获取悬浮球所在屏幕")?;
    let area = monitor.work_area();
    let ((x, y), placement) = placement(
        mode,
        anchor,
        scale,
        [
            area.position.x as f64,
            area.position.y as f64,
            area.size.width as f64,
            area.size.height as f64,
        ],
    );
    window
        .set_size(tauri::LogicalSize::new(placement.width, placement.height))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(
            x.round() as i32,
            y.round() as i32,
        ))
        .map_err(|e| e.to_string())?;
    *state.layout.lock().unwrap() = mode;
    *state.placement.lock().unwrap() = Some(placement);
    Ok(placement)
}
#[tauri::command]
pub fn compact_set_interacting(app: tauri::AppHandle, active: bool) -> Result<(), String> {
    app.state::<Overlay>()
        .interacting
        .store(active, Ordering::SeqCst);
    if active {
        if let Some(window) = app.get_webview_window("compact") {
            window
                .set_ignore_cursor_events(false)
                .map_err(|e| e.to_string())?;
            app.state::<Overlay>()
                .ignored
                .store(false, Ordering::SeqCst);
        }
    }
    Ok(())
}
#[tauri::command]
pub fn compact_drag(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("compact").ok_or("陪伴球尚未打开")?;
    window.start_dragging().map_err(|e| e.to_string())
}
#[tauri::command]
pub fn compact_focus(app: tauri::AppHandle) -> Result<(), String> {
    app.get_webview_window("compact")
        .ok_or("陪伴球尚未打开")?
        .set_focus()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn compact_open_main(app: tauri::AppHandle, settings: Option<bool>) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主窗口不可用")?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    if settings.unwrap_or(false) {
        let _ = window.emit("linger:open-settings", ());
    }
    Ok(())
}

pub fn start_hit_testing(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut audio_tick = 0u8;
        let mut previous_hover = None;
        let mut position_tick = 0u8;
        let mut candidate_position = None;
        let mut saved_position = None;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(40));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(window) = app.get_webview_window("compact") else {
                continue;
            };
            let state = app.state::<Overlay>();
            position_tick = (position_tick + 1) % 10;
            if position_tick == 0 {
                if let Ok(position) = remember_position(&app, &window) {
                    if candidate_position.as_ref() == Some(&position)
                        && saved_position.as_ref() != Some(&position)
                    {
                        let core = app.state::<std::sync::Arc<crate::Core>>();
                        if core.store.set_meta(POSITION_KEY, &position).is_ok() {
                            saved_position = Some(position.clone());
                        }
                    }
                    candidate_position = Some(position);
                }
            }
            if !window.is_visible().unwrap_or(false) {
                state.interacting.store(false, Ordering::SeqCst);
                previous_hover = None;
                continue;
            }
            audio_tick = (audio_tick + 1) % 2;
            let core = app.state::<std::sync::Arc<crate::Core>>();
            if audio_tick == 0 && core.recording.load(Ordering::SeqCst) {
                let _ = window.emit(
                    "linger:audio-level",
                    serde_json::json!({"level":core.devices.input_level()}),
                );
            }
            let interacting = state.interacting.load(Ordering::SeqCst);
            let geometry = (
                app.cursor_position(),
                window.inner_position(),
                window.inner_size(),
                window.scale_factor(),
            );
            let hover = if let (Ok(cursor), Ok(pos), Ok(size), Ok(scale)) = geometry {
                let width = size.width as f64 / scale;
                let height = size.height as f64 / scale;
                let x = (cursor.x - pos.x as f64) / scale;
                let y = (cursor.y - pos.y as f64) / scale;
                let measured = state.hit_regions.lock().unwrap();
                Some(
                    measured
                        .as_ref()
                        .map(|regions| {
                            regions.hit(
                                cursor.x - pos.x as f64,
                                cursor.y - pos.y as f64,
                                size.width as f64,
                                size.height as f64,
                            )
                        })
                        .unwrap_or_else(|| {
                            (
                                circle(x, y, width - 28.0, height - 28.0, 24.0),
                                hit(
                                    *state.layout.lock().unwrap(),
                                    width,
                                    height,
                                    x,
                                    y,
                                    matches!(
                                        core.last_status.lock().unwrap()["phase"].as_str(),
                                        Some("speaking" | "playing" | "synthesizing")
                                    ),
                                ),
                            )
                        }),
                )
            } else {
                None
            };
            let should_ignore = !interacting && hover.is_some_and(|(_, inside)| !inside);
            if state.ignored.load(Ordering::SeqCst) != should_ignore
                && window.set_ignore_cursor_events(should_ignore).is_ok()
            {
                state.ignored.store(should_ignore, Ordering::SeqCst);
            }
            // Restoring native hit testing need not produce a WebView pointerenter.
            // Send hover transitions without activating the app or taking keyboard focus.
            if let Some((orb, inside)) = hover {
                if !interacting && previous_hover != hover {
                    let _ = window.emit(
                        "linger:overlay-hover",
                        serde_json::json!({"orb":orb,"inside":inside}),
                    );
                    previous_hover = hover;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_ball_position_handles_display_changes_and_invalid_values() {
        let p = SavedPosition {
            monitor: Some("external".into()),
            x: 0.75,
            y: 0.5,
        };
        assert_eq!(
            p.anchor([-1920.0, 0.0, 1920.0, 1080.0], 1.0),
            Some((-480.0, 540.0))
        );
        assert_eq!(
            p.anchor([0.0, 0.0, 2560.0, 1440.0], 2.0),
            Some((1920.0, 720.0))
        );
        let out = SavedPosition {
            x: -2.0,
            y: 4.0,
            ..p.clone()
        };
        assert_eq!(
            out.anchor([0.0, 0.0, 1920.0, 1080.0], 2.0),
            Some((56.0, 1024.0))
        );
        assert!(SavedPosition {
            x: f64::NAN,
            ..p.clone()
        }
        .anchor([0.0, 0.0, 1920.0, 1080.0], 1.0)
        .is_none());
        let restored: SavedPosition =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(
            restored.anchor([0.0, 0.0, 1920.0, 1080.0], 1.0),
            p.anchor([0.0, 0.0, 1920.0, 1080.0], 1.0)
        );
    }
    #[test]
    fn transparent_pixels_pass_through_but_orb_controls_and_cards_work() {
        assert!(hit(Layout::Orb, 56.0, 56.0, 28.0, 28.0, false));
        assert!(!hit(Layout::Orb, 56.0, 56.0, 1.0, 1.0, false));
        assert!(!hit(Layout::Notice, 192.0, 88.0, 80.0, 12.0, false));
        assert!(hit(Layout::Tools, 280.0, 72.0, 120.0, 44.0, false));
        assert!(!hit(Layout::Tools, 280.0, 72.0, 225.0, 44.0, false));
        assert!(!hit(Layout::Tools, 280.0, 72.0, 141.0, 44.0, false));
        assert!(hit(Layout::Tools, 280.0, 72.0, 204.0, 44.0, false));
        assert!(hit(Layout::Tools, 280.0, 72.0, 78.0, 44.0, false));
        assert!(hit(Layout::Tools, 280.0, 72.0, 36.0, 44.0, true));
        assert!(!hit(Layout::Tools, 280.0, 72.0, 36.0, 44.0, false));
        assert!(hit(Layout::Orb, 96.0, 56.0, 20.0, 28.0, false));
        assert!(!hit(Layout::Orb, 96.0, 56.0, 40.0, 28.0, false));
        assert!(hit(Layout::Chat, 320.0, 440.0, 150.0, 200.0, false));
        assert!(!hit(Layout::Chat, 320.0, 440.0, 100.0, 400.0, false));
        assert!(hit(Layout::Chat, 320.0, 440.0, 292.0, 412.0, false));
    }
    #[test]
    fn measured_regions_follow_webview_zoom_and_client_origin() {
        let regions = HitRegions {
            width: 240.0,
            height: 72.0,
            regions: vec![
                HitRegion {
                    x: 62.0,
                    y: 20.0,
                    width: 78.0,
                    height: 48.0,
                    radius: 0.0,
                    orb: false,
                },
                HitRegion {
                    x: 188.0,
                    y: 20.0,
                    width: 48.0,
                    height: 48.0,
                    radius: 24.0,
                    orb: true,
                },
            ],
        };
        for scale in [0.75, 1.0, 1.25, 1.5, 2.0] {
            assert_eq!(
                regions.hit(80.0 * scale, 44.0 * scale, 240.0 * scale, 72.0 * scale),
                (false, true)
            );
            assert_eq!(
                regions.hit(122.0 * scale, 44.0 * scale, 240.0 * scale, 72.0 * scale),
                (false, true)
            );
            assert_eq!(
                regions.hit(212.0 * scale, 44.0 * scale, 240.0 * scale, 72.0 * scale),
                (true, true)
            );
            assert_eq!(
                regions.hit(1.0 * scale, 1.0 * scale, 240.0 * scale, 72.0 * scale),
                (false, false)
            );
        }
    }
    #[test]
    fn layouts_keep_orb_at_all_monitor_edges_and_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let work = [-1920.0 * scale, 0.0, 1920.0 * scale, 1080.0 * scale];
            for anchor in [
                (-1892.0 * scale, 28.0 * scale),
                (-28.0 * scale, 28.0 * scale),
                (-1892.0 * scale, 1052.0 * scale),
                (-28.0 * scale, 1052.0 * scale),
                (-960.0 * scale, 540.0 * scale),
            ] {
                for mode in [
                    Layout::Orb,
                    Layout::Notice,
                    Layout::Tools,
                    Layout::Chat,
                    Layout::Translate,
                    Layout::More,
                    Layout::Bubble,
                ] {
                    let ((x, y), p) = placement(mode, anchor, scale, work);
                    assert!((x + p.orb_x * scale - anchor.0).abs() < 0.001);
                    assert!((y + p.orb_y * scale - anchor.1).abs() < 0.001);
                    assert!(x >= work[0] - 0.001 && y >= work[1] - 0.001);
                    assert!(x + p.width * scale <= work[0] + work[2] + 0.001);
                    assert!(y + p.height * scale <= work[1] + work[3] + 0.001);
                }
            }
        }
    }
}
