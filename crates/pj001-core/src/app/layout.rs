use std::collections::HashMap;

use winit::dpi::PhysicalSize;

use crate::render::CellMetrics;

use super::PaneViewport;
use super::event::PaneId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        ratio: SplitRatio,
        primary: Box<Layout>,
        secondary: Box<Layout>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SplitAxis {
    Vertical,
    #[allow(dead_code)]
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SplitRatio(u16);

impl SplitRatio {
    const MAX: u16 = 10_000;

    pub(super) const fn half() -> Self {
        Self(5_000)
    }

    fn primary_units(self, available: usize) -> usize {
        if available <= 1 {
            return available;
        }
        let scaled = available * self.0 as usize;
        let mut primary = scaled.div_ceil(Self::MAX as usize);
        primary = primary.clamp(1, available - 1);
        primary
    }
}

impl Layout {
    pub(super) fn from_initial_panes(panes: &[PaneId]) -> Self {
        match panes {
            [single] => Self::Pane(*single),
            [first, second] => Self::Split {
                axis: SplitAxis::Vertical,
                ratio: SplitRatio::half(),
                primary: Box::new(Self::Pane(*first)),
                secondary: Box::new(Self::Pane(*second)),
            },
            _ => panic!("initial layout currently supports one or two panes"),
        }
    }

    pub(super) fn split_pane(&mut self, target: PaneId, axis: SplitAxis, new_pane: PaneId) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    ratio: SplitRatio::half(),
                    primary: Box::new(Self::Pane(target)),
                    secondary: Box::new(Self::Pane(new_pane)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split {
                primary, secondary, ..
            } => {
                primary.split_pane(target, axis, new_pane)
                    || secondary.split_pane(target, axis, new_pane)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CellRect {
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VerticalDivider {
    pub col: usize,
    pub row: usize,
    pub height: usize,
}

pub(super) fn compute_viewports(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
) -> HashMap<PaneId, PaneViewport> {
    let raw_rows = (size.height / cell.height).max(1) as usize;
    let status_row = matches!(root, Layout::Split { .. })
        .then_some(raw_rows - 1)
        .filter(|_| raw_rows > 1);
    let rows = status_row.unwrap_or(raw_rows);
    let cols = (size.width / cell.width).max(1) as usize;
    let rect = CellRect {
        col: 0,
        row: 0,
        cols,
        rows,
    };
    let mut out = HashMap::new();
    resolve(root, rect, status_row, size, cell, &mut out);
    out
}

pub(super) fn vertical_dividers(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
) -> Vec<VerticalDivider> {
    let raw_rows = (size.height / cell.height).max(1) as usize;
    let status_row = matches!(root, Layout::Split { .. })
        .then_some(raw_rows - 1)
        .filter(|_| raw_rows > 1);
    let rows = status_row.unwrap_or(raw_rows);
    let cols = (size.width / cell.width).max(1) as usize;
    let rect = CellRect {
        col: 0,
        row: 0,
        cols,
        rows,
    };
    let mut out = Vec::new();
    collect_vertical_dividers(root, rect, &mut out);
    out
}

fn resolve(
    node: &Layout,
    rect: CellRect,
    status_row: Option<usize>,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
    out: &mut HashMap<PaneId, PaneViewport>,
) {
    match node {
        Layout::Pane(id) => {
            out.insert(
                *id,
                PaneViewport {
                    cols: rect.cols.max(1),
                    rows: rect.rows.max(1),
                    col_offset: rect.col,
                    status_row,
                    x_px: rect.col as u32 * cell.width,
                    y_px: rect.row as u32 * cell.height,
                    width_px: rect_width_px(rect, size, cell),
                    height_px: rect_height_px(rect, cell),
                },
            );
        }
        Layout::Split {
            axis,
            ratio,
            primary,
            secondary,
        } => {
            let (primary_rect, secondary_rect) = split_rect(rect, *axis, *ratio);
            resolve(primary, primary_rect, status_row, size, cell, out);
            resolve(secondary, secondary_rect, status_row, size, cell, out);
        }
    }
}

fn split_rect(rect: CellRect, axis: SplitAxis, ratio: SplitRatio) -> (CellRect, CellRect) {
    match axis {
        SplitAxis::Vertical => {
            let divider = usize::from(rect.cols >= 3);
            let content = rect.cols.saturating_sub(divider);
            if content <= 1 {
                return (CellRect { cols: 1, ..rect }, CellRect { cols: 1, ..rect });
            }
            let primary_cols = ratio.primary_units(content);
            let secondary_cols = content - primary_cols;
            (
                CellRect {
                    cols: primary_cols,
                    ..rect
                },
                CellRect {
                    col: rect.col + primary_cols + divider,
                    cols: secondary_cols,
                    ..rect
                },
            )
        }
        SplitAxis::Horizontal => {
            let divider = usize::from(rect.rows >= 3);
            let content = rect.rows.saturating_sub(divider);
            if content <= 1 {
                return (CellRect { rows: 1, ..rect }, CellRect { rows: 1, ..rect });
            }
            let primary_rows = ratio.primary_units(content);
            let secondary_rows = content - primary_rows;
            (
                CellRect {
                    rows: primary_rows,
                    ..rect
                },
                CellRect {
                    row: rect.row + primary_rows + divider,
                    rows: secondary_rows,
                    ..rect
                },
            )
        }
    }
}

fn collect_vertical_dividers(node: &Layout, rect: CellRect, out: &mut Vec<VerticalDivider>) {
    match node {
        Layout::Pane(_) => {}
        Layout::Split {
            axis,
            ratio,
            primary,
            secondary,
        } => {
            if *axis == SplitAxis::Vertical && rect.cols >= 3 {
                let content = rect.cols - 1;
                let primary_cols = ratio.primary_units(content);
                out.push(VerticalDivider {
                    col: rect.col + primary_cols,
                    row: rect.row,
                    height: rect.rows,
                });
            }
            let (primary_rect, secondary_rect) = split_rect(rect, *axis, *ratio);
            collect_vertical_dividers(primary, primary_rect, out);
            collect_vertical_dividers(secondary, secondary_rect, out);
        }
    }
}

fn rect_width_px(rect: CellRect, size: PhysicalSize<u32>, cell: CellMetrics) -> u32 {
    if rect.col == 0 && size.width < cell.width {
        return size.width;
    }
    let width = rect.cols as u32 * cell.width;
    if rect.col == 0 && width == 0 {
        size.width
    } else {
        width
    }
}

fn rect_height_px(rect: CellRect, cell: CellMetrics) -> u32 {
    rect.rows as u32 * cell.height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> CellMetrics {
        CellMetrics {
            width: 10,
            height: 20,
            baseline: 15.0,
        }
    }

    fn pane(id: u64) -> Layout {
        Layout::Pane(PaneId(id))
    }

    #[test]
    fn initial_layout_single_is_pane() {
        assert_eq!(
            Layout::from_initial_panes(&[PaneId(7)]),
            Layout::Pane(PaneId(7))
        );
    }

    #[test]
    fn initial_layout_pair_is_vertical_half_split() {
        assert_eq!(
            Layout::from_initial_panes(&[PaneId(1), PaneId(2)]),
            Layout::Split {
                axis: SplitAxis::Vertical,
                ratio: SplitRatio::half(),
                primary: Box::new(Layout::Pane(PaneId(1))),
                secondary: Box::new(Layout::Pane(PaneId(2))),
            }
        );
    }

    #[test]
    fn split_pane_replaces_target_with_split() {
        let mut root = Layout::Pane(PaneId(1));

        assert!(root.split_pane(PaneId(1), SplitAxis::Vertical, PaneId(2)));
        assert_eq!(
            root,
            Layout::Split {
                axis: SplitAxis::Vertical,
                ratio: SplitRatio::half(),
                primary: Box::new(Layout::Pane(PaneId(1))),
                secondary: Box::new(Layout::Pane(PaneId(2))),
            }
        );
    }

    #[test]
    fn split_pane_finds_nested_target() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);

        assert!(root.split_pane(PaneId(2), SplitAxis::Vertical, PaneId(3)));
        let viewports = compute_viewports(&root, PhysicalSize::new(120, 80), cell());

        assert_eq!(viewports.len(), 3);
        assert!(viewports[&PaneId(3)].col_offset > viewports[&PaneId(2)].col_offset);
    }

    #[test]
    fn split_pane_returns_false_for_missing_target() {
        let mut root = Layout::Pane(PaneId(1));

        assert!(!root.split_pane(PaneId(99), SplitAxis::Vertical, PaneId(2)));
        assert_eq!(root, Layout::Pane(PaneId(1)));
    }

    #[test]
    fn single_pane_uses_full_window() {
        let viewports = compute_viewports(&pane(0), PhysicalSize::new(100, 80), cell());
        let vp = viewports[&PaneId(0)];

        assert_eq!(vp.cols, 10);
        assert_eq!(vp.rows, 4);
        assert_eq!(vp.col_offset, 0);
        assert_eq!(vp.status_row, None);
        assert_eq!(vp.width_px, 100);
        assert_eq!(vp.height_px, 80);
    }

    #[test]
    fn vertical_split_reserves_divider_column_when_space_allows() {
        let root = Layout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(pane(1)),
        };
        let viewports = compute_viewports(&root, PhysicalSize::new(100, 80), cell());

        assert_eq!(viewports[&PaneId(0)].cols, 5);
        assert_eq!(viewports[&PaneId(0)].col_offset, 0);
        assert_eq!(viewports[&PaneId(1)].cols, 4);
        assert_eq!(viewports[&PaneId(1)].col_offset, 6);
    }

    #[test]
    fn vertical_split_drops_divider_when_only_two_columns_fit() {
        let root = Layout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(pane(1)),
        };
        let viewports = compute_viewports(&root, PhysicalSize::new(20, 80), cell());

        assert_eq!(viewports[&PaneId(0)].cols, 1);
        assert_eq!(viewports[&PaneId(0)].col_offset, 0);
        assert_eq!(viewports[&PaneId(1)].cols, 1);
        assert_eq!(viewports[&PaneId(1)].col_offset, 1);
    }

    #[test]
    fn vertical_split_overlaps_when_only_one_column_fits() {
        let root = Layout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(pane(1)),
        };
        let viewports = compute_viewports(&root, PhysicalSize::new(1, 80), cell());

        assert_eq!(viewports[&PaneId(0)].cols, 1);
        assert_eq!(viewports[&PaneId(0)].col_offset, 0);
        assert_eq!(viewports[&PaneId(1)].cols, 1);
        assert_eq!(viewports[&PaneId(1)].col_offset, 0);
    }

    #[test]
    fn horizontal_split_reserves_divider_row_when_space_allows() {
        let root = Layout::Split {
            axis: SplitAxis::Horizontal,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(pane(1)),
        };
        let viewports = compute_viewports(&root, PhysicalSize::new(100, 120), cell());

        assert_eq!(viewports[&PaneId(0)].rows, 2);
        assert_eq!(viewports[&PaneId(0)].y_px, 0);
        assert_eq!(viewports[&PaneId(1)].rows, 2);
        assert_eq!(viewports[&PaneId(1)].y_px, 60);
    }

    #[test]
    fn nested_split_computes_three_panes() {
        let root = Layout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(Layout::Split {
                axis: SplitAxis::Horizontal,
                ratio: SplitRatio::half(),
                primary: Box::new(pane(1)),
                secondary: Box::new(pane(2)),
            }),
        };
        let viewports = compute_viewports(&root, PhysicalSize::new(100, 120), cell());

        assert_eq!(viewports.len(), 3);
        assert_eq!(viewports[&PaneId(0)].cols, 5);
        assert_eq!(viewports[&PaneId(1)].col_offset, 6);
        assert_eq!(viewports[&PaneId(2)].col_offset, 6);
        assert!(viewports[&PaneId(2)].y_px > viewports[&PaneId(1)].y_px);
    }

    #[test]
    fn vertical_dividers_include_nested_vertical_split() {
        let root = Layout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(Layout::Split {
                axis: SplitAxis::Vertical,
                ratio: SplitRatio::half(),
                primary: Box::new(pane(1)),
                secondary: Box::new(pane(2)),
            }),
        };
        let dividers = vertical_dividers(&root, PhysicalSize::new(120, 80), cell());

        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].col, 6);
        assert!(dividers[1].col > dividers[0].col);
    }
}
