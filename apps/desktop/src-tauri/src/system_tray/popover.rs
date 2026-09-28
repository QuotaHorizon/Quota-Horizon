use tauri::{
    webview::Color, AppHandle, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};
use tauri::{Emitter, Manager};

const LABEL: &str = "capacity-popover";
const WIDTH: f64 = 388.0;
const HEIGHT: f64 = 452.0;
const SCREEN_MARGIN: f64 = 10.0;
const ANCHOR_GAP: f64 = 8.0;

pub(super) fn is_label(label: &str) -> bool {
    label == LABEL
}

pub(super) fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}

pub(super) fn toggle<R: Runtime>(app: &AppHandle<R>, anchor: PhysicalPosition<f64>) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            return;
        }
        let _ = position_window(app, &window, anchor);
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit("capacity-popover-opened", ());
        return;
    }

    let window = match WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("index.html#capacity-popover".into()),
    )
    .title("QuotaHorizon Capacity")
    .inner_size(WIDTH, HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    .transparent(true)
    .background_color(Color(0, 0, 0, 0))
    // Transparent borderless windows can receive a rectangular AppKit shadow
    // in addition to the card's CSS shadow. The two masks do not share the
    // same rounded edge and produce a dark, irregular fringe on macOS.
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(true)
    .visible(false)
    .build()
    {
        Ok(window) => window,
        Err(error) => {
            eprintln!("failed to create capacity popover: {error}");
            return;
        }
    };
    if let Err(error) = position_window(app, &window, anchor) {
        eprintln!("failed to position capacity popover: {error}");
    }
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit("capacity-popover-opened", ());
}

fn position_window<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    anchor: PhysicalPosition<f64>,
) -> Result<(), String> {
    let monitor = app
        .available_monitors()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|monitor| point_inside(anchor, *monitor.position(), *monitor.size()))
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or_else(|| "no display is available for the capacity popover".to_owned())?;
    let position = bounded_position(
        anchor,
        monitor.work_area().position,
        monitor.work_area().size,
        window.outer_size().map_err(|error| error.to_string())?,
    );
    window
        .set_position(position)
        .map_err(|error| error.to_string())
}

fn point_inside(
    point: PhysicalPosition<f64>,
    origin: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) -> bool {
    point.x >= f64::from(origin.x)
        && point.y >= f64::from(origin.y)
        && point.x < f64::from(origin.x) + f64::from(size.width)
        && point.y < f64::from(origin.y) + f64::from(size.height)
}

fn bounded_position(
    anchor: PhysicalPosition<f64>,
    origin: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    window_size: PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let left = f64::from(origin.x) + SCREEN_MARGIN;
    let top = f64::from(origin.y) + SCREEN_MARGIN;
    let right = f64::from(origin.x) + f64::from(size.width) - SCREEN_MARGIN;
    let bottom = f64::from(origin.y) + f64::from(size.height) - SCREEN_MARGIN;
    let window_width = f64::from(window_size.width);
    let window_height = f64::from(window_size.height);
    let x = (anchor.x - window_width / 2.0).clamp(left, (right - window_width).max(left));
    let below = anchor.y + ANCHOR_GAP;
    let above = anchor.y - window_height - ANCHOR_GAP;
    let y = if below + window_height <= bottom {
        below.max(top)
    } else {
        above.clamp(top, (bottom - window_height).max(top))
    };
    PhysicalPosition::new(x.round() as i32, y.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popover_centers_below_a_top_menu_anchor() {
        let position = bounded_position(
            PhysicalPosition::new(900.0, 24.0),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1_920, 1_080),
            PhysicalSize::new(WIDTH as u32, HEIGHT as u32),
        );

        assert_eq!(position, PhysicalPosition::new(706, 32));
    }

    #[test]
    fn popover_stays_inside_small_and_offset_displays() {
        let position = bounded_position(
            PhysicalPosition::new(-1_280.0, 700.0),
            PhysicalPosition::new(-1_440, 100),
            PhysicalSize::new(1_440, 900),
            PhysicalSize::new(WIDTH as u32, HEIGHT as u32),
        );

        assert!(position.x >= -1_430);
        assert!(position.x + WIDTH as i32 <= -10);
        assert!(position.y >= 110);
        assert!(position.y + HEIGHT as i32 <= 990);
    }

    #[test]
    fn popover_uses_physical_window_size_on_retina_displays() {
        let position = bounded_position(
            PhysicalPosition::new(1_800.0, 48.0),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(3_840, 2_160),
            PhysicalSize::new(776, 904),
        );

        assert_eq!(position, PhysicalPosition::new(1_412, 56));
        assert!(position.x + 776 <= 3_830);
        assert!(position.y + 904 <= 2_150);
    }
}
