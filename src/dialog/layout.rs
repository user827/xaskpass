use log::trace;
use serde::{Deserialize, Serialize};

use super::{Buttons, Indicator, Label};
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
    ) -> fn(&config::Layout, &mut Label, &mut Buttons, &mut Indicator) -> (f64, f64) {
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
    buttons: &mut Buttons,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    indicator.for_width(buttons.ok().width);
    let buttonind_area_width =
        (4.0 * horizontal_spacing) + buttons.ok().width + buttons.cancel().width + indicator.width;
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
    let inter_buttonind_space = ((width - buttons.ok().width - indicator.width) / 3.0).floor();
    indicator.x = inter_buttonind_space;
    label.x = horizontal_spacing;
    buttons.ok().x = width - horizontal_spacing - buttons.ok().width;
    buttons.cancel().x = buttons.ok().x;

    let vertical_spacing = config.vertical_spacing;
    let button_area_height: f64 = vertical_spacing + buttons.ok().height + buttons.cancel().height;
    let buttonind_area_height = button_area_height.max(indicator.height);
    let space = vertical_spacing;
    let height = (2.0 * vertical_spacing) + label.height + buttonind_area_height + space;
    label.y = vertical_spacing;
    buttons.ok().y = label.y + label.height + space;
    indicator.y =
        buttons.ok().y + (height - buttons.ok().y - indicator.height - vertical_spacing) / 2.0;
    buttons.cancel().y = buttons.ok().y + buttons.ok().height + vertical_spacing;

    (width, height)
}

pub fn middle_compact(
    config: &config::Layout,
    label: &mut Label,
    buttons: &mut Buttons,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    indicator.for_width(buttons.ok().width);
    let buttonind_area_width =
        (8.0 * horizontal_spacing) + buttons.ok().width + buttons.cancel().width + indicator.width;
    label.calc_extents(config.text_width, true);
    let label_area_width = label.width + (2.0 * horizontal_spacing);
    let width = label_area_width.max(buttonind_area_width);
    label.x = (width - label.width) / 2.0;
    let inter_space = (width - buttons.ok().width - buttons.cancel().width - indicator.width) / 4.0;
    buttons.ok().x = inter_space;
    indicator.x = buttons.ok().x + buttons.ok().width + inter_space;
    buttons.cancel().x = indicator.x + indicator.width + inter_space;

    let vertical_spacing = config.vertical_spacing;
    let buttonind_area_height = buttons
        .ok()
        .height
        .max(buttons.cancel().height)
        .max(indicator.height);
    let height = (3.0 * vertical_spacing) + label.height + buttonind_area_height;
    label.y = vertical_spacing;
    buttons.ok().y = height - vertical_spacing - buttons.ok().height;
    buttons.cancel().y = buttons.ok().y;
    indicator.y = height - vertical_spacing - buttonind_area_height
        + (buttonind_area_height - indicator.height) / 2.0;
    trace!(
        "buttonind_area_height: {}, indicator.height: {}, buttons.ok().height: {}",
        buttonind_area_height,
        indicator.height,
        buttons.ok().height
    );
    (width, height)
}

pub fn center(
    config: &config::Layout,
    label: &mut Label,
    buttons: &mut Buttons,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing = config.horizontal_spacing;
    let button_area_width =
        (3.0 * horizontal_spacing) + buttons.ok().width + buttons.cancel().width;
    label.calc_extents(config.text_width, true);
    let label_area_width = label.width + (2.0 * horizontal_spacing);
    let w = label_area_width.max(button_area_width);
    let indicator_spacing = 4.0;
    indicator
        .for_width(w - 2.0 * horizontal_spacing - buttons.clipboard().width - indicator_spacing);
    let indicator_area_width =
        indicator.width + 2.0 * horizontal_spacing + buttons.clipboard().width + indicator_spacing;
    let width = w.max(indicator_area_width);

    indicator.x = ((width - indicator.width) / 2.0).floor();
    if indicator.x < buttons.clipboard().width + horizontal_spacing + indicator_spacing {
        buttons.clipboard().x = width - buttons.clipboard().width - horizontal_spacing;
        indicator.x = buttons.clipboard().x - indicator_spacing - indicator.width;
    } else {
        buttons.clipboard().x = indicator.x + indicator.width + indicator_spacing;
    }
    // floor instead of round so these stay within the widths specified above
    label.x = ((width - label.width) / 2.0).floor();
    let inter_button_space = ((width - buttons.ok().width - buttons.cancel().width) / 3.0).floor();
    buttons.ok().x = inter_button_space;
    buttons.cancel().x = buttons.ok().x + buttons.ok().width + inter_button_space;

    let vertical_spacing = config.vertical_spacing;
    let indicator_area_height = indicator.height.max(buttons.clipboard().height);
    let height =
        (4.0 * vertical_spacing) + label.height + indicator_area_height + buttons.ok().height;

    label.y = vertical_spacing;
    let indicator_area_y = label.y + label.height + vertical_spacing;
    indicator.y = indicator_area_y + ((indicator_area_height - indicator.height) / 2.0).floor();
    buttons.clipboard().y =
        indicator_area_y + ((indicator_area_height - buttons.clipboard().height) / 2.0).floor();
    buttons.ok().y = indicator_area_y + indicator_area_height + vertical_spacing;
    buttons.cancel().y = buttons.ok().y;

    (width, height)
}

pub fn top_right(
    config: &config::Layout,
    label: &mut Label,
    buttons: &mut Buttons,
    indicator: &mut Indicator,
) -> (f64, f64) {
    let horizontal_spacing: f64 = config.horizontal_spacing;
    // debug!("label width {}", label.width);
    let hspace = 2.0 * horizontal_spacing;
    indicator.for_width(buttons.ok().width);
    label.calc_extents(
        Some(
            config
                .text_width
                .unwrap_or((buttons.ok().width + buttons.cancel().width).round() as u32),
        ),
        true,
    );
    let label_area_width = label.width + (4.0 * horizontal_spacing) + indicator.width + hspace;
    // debug!("label area width {}", label_area_width);
    let button_area_width =
        (3.0 * horizontal_spacing) + buttons.ok().width + buttons.cancel().width;
    let width = label_area_width.max(button_area_width);
    label.x = horizontal_spacing * 2.0;
    indicator.x = width - horizontal_spacing * 2.0 - indicator.width;
    buttons.ok().x = width - horizontal_spacing - buttons.ok().width;
    buttons.cancel().x = buttons.ok().x - horizontal_spacing - buttons.cancel().width;

    let vertical_spacing = config.vertical_spacing;
    let label_area_height = label.height.max(indicator.height);
    let vspace = 3.0 * vertical_spacing;
    let height = (2.0 * vertical_spacing) + label_area_height + buttons.ok().height + vspace;
    label.y = vertical_spacing;
    indicator.y = label.y;
    buttons.ok().y = label.y + label_area_height + vspace;
    buttons.cancel().y = buttons.ok().y;

    (width, height)
}
