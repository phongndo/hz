pub(crate) mod diff;
pub(crate) mod grep;
pub(crate) mod headers;
pub(crate) mod menus;
pub(crate) mod sidebar;
pub(crate) mod statusline;
pub(crate) mod style;
pub(crate) mod text;

use crate::app::DiffApp;
use ratatui::{Frame, layout::Rect};

use self::{
    diff::draw_diff,
    menus::{draw_branch_menu, draw_diff_menu, draw_help_menu},
    sidebar::{draw_file_sidebar, file_sidebar_width},
    statusline::{
        draw_error_log, draw_filter_bar, draw_header, error_log_height, filter_bar_visible,
    },
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut DiffApp) {
    let area = frame.area();
    if area.height == 0 {
        return;
    }

    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let filter_bar_height = u16::from(area.height > 1 && filter_bar_visible(app));
    let error_log_height = error_log_height(
        app,
        area.height
            .saturating_sub(1)
            .saturating_sub(filter_bar_height),
    );
    let body_height = area
        .height
        .saturating_sub(1)
        .saturating_sub(filter_bar_height)
        .saturating_sub(error_log_height);
    let body_area = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: area.width,
        height: body_height,
    };
    let filter_bar_area = (filter_bar_height > 0).then_some(Rect {
        x: area.x,
        y: body_area.y.saturating_add(body_area.height),
        width: area.width,
        height: 1,
    });
    let error_log_area = (error_log_height > 0).then_some(Rect {
        x: area.x,
        y: body_area
            .y
            .saturating_add(body_area.height)
            .saturating_add(filter_bar_height),
        width: area.width,
        height: error_log_height,
    });

    let sidebar_width = file_sidebar_width(app, body_area.width);
    app.file_sidebar_render_width = sidebar_width;
    let (sidebar_area, diff_area) = if sidebar_width > 0 {
        (
            Some(Rect {
                x: body_area.x,
                y: body_area.y,
                width: sidebar_width,
                height: body_area.height,
            }),
            Rect {
                x: body_area.x.saturating_add(sidebar_width),
                y: body_area.y,
                width: body_area.width.saturating_sub(sidebar_width),
                height: body_area.height,
            },
        )
    } else {
        (None, body_area)
    };

    app.set_viewport_rows(diff_area.height as usize);
    app.set_viewport_width(diff_area.width as usize);
    draw_header(frame, app, header_area);
    if let Some(sidebar_area) = sidebar_area {
        draw_file_sidebar(frame, app, sidebar_area);
    }
    draw_diff(frame, app, diff_area);
    if let Some(filter_bar_area) = filter_bar_area {
        draw_filter_bar(frame, app, filter_bar_area);
    }
    if let Some(error_log_area) = error_log_area {
        draw_error_log(frame, app, error_log_area);
    }
    draw_diff_menu(frame, app, area);
    draw_branch_menu(frame, app, area);
    draw_help_menu(frame, app, area);
}
