pub mod bite_detector;
pub mod capture;
pub mod detector;

pub use bite_detector::{BiteReason, BiteStatus, BobberBiteDetector};
pub use capture::{
    capture_primary_screen, capture_wow_window, crop_roi, get_cursor_pos, get_fishing_water_roi,
    get_water_center_640, get_water_center_wide, set_cursor_pos, smooth_move_cursor, WindowCapture, WaterTile,
};
pub use detector::{DetectionBox, VisionDetector, YoloDetector};
