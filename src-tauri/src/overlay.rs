//! Small transparent desktop surface. Only painted controls intercept pointer input.
use serde::Deserialize;
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
            Self::Tools => (240.0, 72.0),
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
        Layout::Orb => false,
        Layout::Tools => (0..if speaking { 3 } else { 2 })
            .any(|i| circle(x, y, width - 118.0 - 42.0 * i as f64, height - 28.0, 18.0)),
        _ => rounded_rect(x, y, 4.0, 4.0, width - 4.0, height - 64.0, 16.0),
    }
}
fn clamp_origin(x: f64, y: f64, width: f64, height: f64, work: [f64; 4]) -> (f64, f64) {
    (
        x.clamp(work[0], (work[0] + work[2] - width).max(work[0])),
        y.clamp(work[1], (work[1] + work[3] - height).max(work[1])),
    )
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
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn compact_set_layout(app: tauri::AppHandle, mode: Layout) -> Result<(), String> {
    let window = app.get_webview_window("compact").ok_or("陪伴球尚未打开")?;
    let state = app.state::<Overlay>();
    let mut layout = state.layout.lock().unwrap();
    if *layout == mode {
        return Ok(());
    }
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let old_size = window.inner_size().map_err(|e| e.to_string())?;
    let old_pos = window.outer_position().map_err(|e| e.to_string())?;
    let (width, height) = mode.size();
    let mut x = old_pos.x as f64 + old_size.width as f64 - width * scale;
    let mut y = old_pos.y as f64 + old_size.height as f64 - height * scale;
    if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
        let area = monitor.work_area();
        (x, y) = clamp_origin(
            x,
            y,
            width * scale,
            height * scale,
            [
                area.position.x as f64,
                area.position.y as f64,
                area.size.width as f64,
                area.size.height as f64,
            ],
        );
    }
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    window
        .set_position(tauri::PhysicalPosition::new(x as i32, y as i32))
        .map_err(|e| e.to_string())?;
    *layout = mode;
    Ok(())
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
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(40));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(window) = app.get_webview_window("compact") else {
                continue;
            };
            let state = app.state::<Overlay>();
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
    fn transparent_pixels_pass_through_but_orb_controls_and_cards_work() {
        assert!(hit(Layout::Orb, 56.0, 56.0, 28.0, 28.0, false));
        assert!(!hit(Layout::Orb, 56.0, 56.0, 1.0, 1.0, false));
        assert!(hit(Layout::Tools, 240.0, 72.0, 80.0, 44.0, false));
        assert!(!hit(Layout::Tools, 240.0, 72.0, 185.0, 44.0, false));
        assert!(!hit(Layout::Tools, 240.0, 72.0, 101.0, 44.0, false));
        assert!(hit(Layout::Tools, 240.0, 72.0, 164.0, 44.0, false));
        assert!(hit(Layout::Tools, 240.0, 72.0, 38.0, 44.0, true));
        assert!(!hit(Layout::Tools, 240.0, 72.0, 38.0, 44.0, false));
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
    fn bounds_support_negative_monitor_coordinates_and_large_panels() {
        assert_eq!(
            clamp_origin(-100.0, 900.0, 320.0, 440.0, [-1920.0, 0.0, 1920.0, 1080.0]),
            (-320.0, 640.0)
        );
        assert_eq!(
            clamp_origin(0.0, 0.0, 320.0, 440.0, [50.0, 40.0, 100.0, 100.0]),
            (50.0, 40.0)
        );
    }
}
