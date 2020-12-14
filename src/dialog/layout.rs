use log::trace;
use serde::{Deserialize, Serialize};

use super::{Button, Indicator, Label};
use crate::config;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Layout {
    BottomLeft,
    Center,
    MiddleCompact,
    TopRight,
}

impl Layout {
    pub fn get_fn(
        &self,
    ) -> fn(
        &config::Layout,
        &mut Label,
        &mut Button,
        &mut Button,
        &mut Indicator,
        &mut Button,
    ) -> (f64, f64) {
        match self {
            Layout::TopRight => top_right,
            Layout::Center => center,
            Layout::BottomLeft => bottom_left,
            Layout::MiddleCompact => middle_compact,
        }
    }
}

pub fn bottom_left(
    config: &config::Layout,
    label: &mut Label,
    ok_button: &mut Button,
    cancel_button: &mut Button,
    indicator: &mut Indicator,
    clipboard_button: &mut Button,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    indicator.for_width(ok_button.width);
    let buttonind_area_width =
        (4.0 * horizontal_spacing) + ok_button.width + cancel_button.width + indicator.width;
    label.calc_extents(
        Some(
            config
                .text_width
                .unwrap_or(buttonind_area_width.round() as u32),
        ),
        true,
    );
    let label_area_width = label.width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    // floor instead of round so these stay within the widths specified above
    let inter_buttonind_space = ((width - ok_button.width - indicator.width) / 3.0).floor();
    indicator.x = inter_buttonind_space;
    label.x = horizontal_spacing;
    ok_button.x = width - horizontal_spacing - ok_button.width;
    cancel_button.x = ok_button.x;

    let vertical_spacing = config.vertical_spacing;
    let button_area_height: f64 = vertical_spacing + ok_button.height + cancel_button.height;
    let buttonind_area_height = button_area_height.max(indicator.height);
    let space = vertical_spacing;
    let height = (2.0 * vertical_spacing) + label.height + buttonind_area_height + space;
    label.y = vertical_spacing;
    ok_button.y = label.y + label.height + space;
    indicator.y = ok_button.y + (height - ok_button.y - indicator.height - vertical_spacing) / 2.0;
    cancel_button.y = ok_button.y + ok_button.height + vertical_spacing;

    (width, height)
}

pub fn middle_compact(
    config: &config::Layout,
    label: &mut Label,
    ok_button: &mut Button,
    cancel_button: &mut Button,
    indicator: &mut Indicator,
    clipboard_button: &mut Button,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    indicator.for_width(ok_button.width);
    let buttonind_area_width =
        (8.0 * horizontal_spacing) + ok_button.width + cancel_button.width + indicator.width;
    label.calc_extents(config.text_width, true);
    let label_area_width = label.width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    label.x = (width - label.width) / 2.0;
    let inter_space = (width - ok_button.width - cancel_button.width - indicator.width) / 4.0;
    ok_button.x = inter_space;
    indicator.x = ok_button.x + ok_button.width + inter_space;
    cancel_button.x = indicator.x + indicator.width + inter_space;

    let vertical_spacing = config.vertical_spacing;
    let buttonind_area_height = ok_button
        .height
        .max(cancel_button.height)
        .max(indicator.height);
    let height = (3.0 * vertical_spacing) + label.height + buttonind_area_height;
    label.y = vertical_spacing;
    ok_button.y = height - vertical_spacing - ok_button.height;
    cancel_button.y = ok_button.y;
    indicator.y = height - vertical_spacing - buttonind_area_height
        + (buttonind_area_height - indicator.height) / 2.0;
    trace!(
        "buttonind_area_height: {}, indicator.height: {}, ok_button.height: {}",
        buttonind_area_height,
        indicator.height,
        ok_button.height
    );
    (width, height)
}

pub fn center(
    config: &config::Layout,
    label: &mut Label,
    ok_button: &mut Button,
    cancel_button: &mut Button,
    indicator: &mut Indicator,
    clipboard_button: &mut Button,
) -> (f64, f64) {
    let horizontal_spacing = config.horizontal_spacing;
    let button_area_width = (3.0 * horizontal_spacing) + ok_button.width + cancel_button.width;
    label.calc_extents(config.text_width, true);
    let label_area_width = label.width + (2.0 * horizontal_spacing);
    let w = label_area_width.max(button_area_width);
    let indicator_spacing = 4.0;
    indicator.for_width(w - 2.0 * horizontal_spacing - clipboard_button.width - indicator_spacing);
    let indicator_area_width =
        indicator.width + 2.0 * horizontal_spacing + clipboard_button.width + indicator_spacing;
    let width = w.max(indicator_area_width);

    indicator.x = ((width - indicator.width) / 2.0).floor();
    if indicator.x < clipboard_button.width + horizontal_spacing + indicator_spacing {
        clipboard_button.x = width - clipboard_button.width - horizontal_spacing;
        indicator.x = clipboard_button.x - indicator_spacing - indicator.width;
    } else {
        clipboard_button.x = indicator.x + indicator.width + indicator_spacing;
    }
    // floor instead of round so these stay within the widths specified above
    label.x = ((width - label.width) / 2.0).floor();
    let inter_button_space = ((width - ok_button.width - cancel_button.width) / 3.0).floor();
    ok_button.x = inter_button_space;
    cancel_button.x = ok_button.x + ok_button.width + inter_button_space;

    let vertical_spacing = config.vertical_spacing;
    let indicator_area_height = indicator.height.max(clipboard_button.height);
    let height = (4.0 * vertical_spacing) + label.height + indicator_area_height + ok_button.height;

    label.y = vertical_spacing;
    let indicator_area_y = label.y + label.height + vertical_spacing;
    indicator.y = indicator_area_y + ((indicator_area_height - indicator.height) / 2.0).floor();
    clipboard_button.y =
        indicator_area_y + ((indicator_area_height - clipboard_button.height) / 2.0).floor();
    ok_button.y = indicator_area_y + indicator_area_height + vertical_spacing;
    cancel_button.y = ok_button.y;

    (width, height)
}

pub fn top_right(
    config: &config::Layout,
    label: &mut Label,
    ok_button: &mut Button,
    cancel_button: &mut Button,
    indicator: &mut Indicator,
    clipboard_button: &mut Button,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    // debug!("label width {}", label.width);
    let hspace = 2.0 * horizontal_spacing;
    indicator.for_width(ok_button.width);
    label.calc_extents(
        Some(
            config
                .text_width
                .unwrap_or((ok_button.width + cancel_button.width).round() as u32),
        ),
        true,
    );
    let label_area_width = label.width + (4.0 * horizontal_spacing) + indicator.width + hspace;
    // debug!("label area width {}", label_area_width);
    let button_area_width = (3.0 * horizontal_spacing) + ok_button.width + cancel_button.width;
    let width = label_area_width.max(button_area_width);
    label.x = horizontal_spacing * 2.0;
    indicator.x = width - horizontal_spacing * 2.0 - indicator.width;
    ok_button.x = width - horizontal_spacing - ok_button.width;
    cancel_button.x = ok_button.x - horizontal_spacing - cancel_button.width;

    let vertical_spacing = config.vertical_spacing;
    let label_area_height = label.height.max(indicator.height);
    let vspace = 3.0 * vertical_spacing;
    let height = (2.0 * vertical_spacing) + label_area_height + ok_button.height + vspace;
    label.y = vertical_spacing;
    indicator.y = label.y;
    ok_button.y = label.y + label_area_height + vspace;
    cancel_button.y = ok_button.y;

    (width, height)
}
