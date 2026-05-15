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
    const MIN: u16 = 1_000;
    const STEP: u16 = 500;

    pub(super) const fn half() -> Self {
        Self(5_000)
    }

    fn adjust(self, delta: i16) -> Self {
        if delta < 0 {
            Self(self.0.saturating_sub(delta.unsigned_abs()).max(Self::MIN))
        } else {
            Self(
                self.0
                    .saturating_add(delta as u16)
                    .min(Self::MAX - Self::MIN),
            )
        }
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

    fn from_primary_units(primary: usize, available: usize) -> Self {
        if available <= 1 {
            return Self::half();
        }
        let scaled = primary.saturating_mul(Self::MAX as usize) / available;
        Self(scaled.clamp(Self::MIN as usize, (Self::MAX - Self::MIN) as usize) as u16)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RatioDirection {
    GrowActive,
    ShrinkActive,
}

impl Layout {
    pub(super) fn from_initial_panes(panes: &[PaneId]) -> Self {
        match panes {
            [] => panic!("initial layout requires at least one pane"),
            [first, rest @ ..] => {
                rest.iter()
                    .fold(Self::Pane(*first), |layout, pane| Self::Split {
                        axis: SplitAxis::Vertical,
                        ratio: SplitRatio::half(),
                        primary: Box::new(layout),
                        secondary: Box::new(Self::Pane(*pane)),
                    })
            }
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

    pub(super) fn close_pane(&mut self, target: PaneId) -> bool {
        match self {
            Self::Pane(_) => false,
            Self::Split {
                primary, secondary, ..
            } => {
                if primary.is_pane(target) {
                    *self = (**secondary).clone();
                    true
                } else if secondary.is_pane(target) {
                    *self = (**primary).clone();
                    true
                } else {
                    primary.close_pane(target) || secondary.close_pane(target)
                }
            }
        }
    }

    fn is_pane(&self, target: PaneId) -> bool {
        matches!(self, Self::Pane(id) if *id == target)
    }

    pub(super) fn pane_order(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    pub(super) fn adjust_split_for_pane(
        &mut self,
        target: PaneId,
        axis: SplitAxis,
        direction: RatioDirection,
    ) -> bool {
        match self {
            Self::Pane(_) => false,
            Self::Split {
                axis: split_axis,
                ratio,
                primary,
                secondary,
            } => {
                let target_in_primary = primary.contains_pane(target);
                let target_in_secondary = secondary.contains_pane(target);
                if *split_axis == axis && (target_in_primary || target_in_secondary) {
                    let delta = match (target_in_primary, direction) {
                        (true, RatioDirection::GrowActive)
                        | (false, RatioDirection::ShrinkActive) => SplitRatio::STEP as i16,
                        (true, RatioDirection::ShrinkActive)
                        | (false, RatioDirection::GrowActive) => -(SplitRatio::STEP as i16),
                    };
                    *ratio = ratio.adjust(delta);
                    true
                } else {
                    primary.adjust_split_for_pane(target, axis, direction)
                        || secondary.adjust_split_for_pane(target, axis, direction)
                }
            }
        }
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split {
                primary, secondary, ..
            } => {
                primary.collect_panes(out);
                secondary.collect_panes(out);
            }
        }
    }

    fn contains_pane(&self, target: PaneId) -> bool {
        match self {
            Self::Pane(id) => *id == target,
            Self::Split {
                primary, secondary, ..
            } => primary.contains_pane(target) || secondary.contains_pane(target),
        }
    }

    pub(super) fn minimum_size(&self, leaf_cols: usize, leaf_rows: usize) -> (usize, usize) {
        match self {
            Self::Pane(_) => (leaf_cols, leaf_rows),
            Self::Split {
                axis,
                primary,
                secondary,
                ..
            } => {
                let (primary_cols, primary_rows) = primary.minimum_size(leaf_cols, leaf_rows);
                let (secondary_cols, secondary_rows) = secondary.minimum_size(leaf_cols, leaf_rows);
                match axis {
                    SplitAxis::Vertical => (
                        primary_cols + secondary_cols + 1,
                        primary_rows.max(secondary_rows),
                    ),
                    SplitAxis::Horizontal => (
                        primary_cols.max(secondary_cols),
                        primary_rows + secondary_rows + 1,
                    ),
                }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HorizontalDivider {
    pub col: usize,
    pub row: usize,
    pub width: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DividerHit {
    axis: SplitAxis,
    path: Vec<SplitBranch>,
}

impl DividerHit {
    pub(super) fn axis(&self) -> SplitAxis {
        self.axis
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitBranch {
    Primary,
    Secondary,
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

pub(super) fn horizontal_dividers(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
) -> Vec<HorizontalDivider> {
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
    collect_horizontal_dividers(root, rect, &mut out);
    out
}

pub(super) fn divider_hit_at(
    root: &Layout,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
    col: usize,
    row: usize,
) -> Option<DividerHit> {
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
    let mut path = Vec::new();
    divider_hit_at_rect(root, rect, col, row, &mut path)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn set_split_ratio_at_cell(
    root: &mut Layout,
    hit: &DividerHit,
    size: PhysicalSize<u32>,
    cell: CellMetrics,
    col: usize,
    row: usize,
    min_leaf_cols: usize,
    min_leaf_rows: usize,
) -> bool {
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
    set_split_ratio_at_rect(
        root,
        rect,
        &hit.path,
        col,
        row,
        min_leaf_cols,
        min_leaf_rows,
    )
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
            let x_px = rect.col as u32 * cell.width;
            out.insert(
                *id,
                PaneViewport {
                    cols: rect.cols.max(1),
                    rows: rect.rows.max(1),
                    col_offset: rect.col,
                    row_offset: rect.row,
                    status_row,
                    x_px,
                    y_px: rect.row as u32 * cell.height,
                    width_px: rect_width_px(rect, size, cell),
                    height_px: rect_height_px(rect, cell),
                    // Phase 4a — gutter_px=0 강제. 4b에서 latch && block_mode=auto면 발동.
                    gutter_px: 0,
                    content_x_px: x_px,
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

fn divider_hit_at_rect(
    node: &Layout,
    rect: CellRect,
    col: usize,
    row: usize,
    path: &mut Vec<SplitBranch>,
) -> Option<DividerHit> {
    match node {
        Layout::Pane(_) => None,
        Layout::Split {
            axis,
            ratio,
            primary,
            secondary,
        } => {
            match axis {
                SplitAxis::Vertical if rect.cols >= 3 => {
                    let divider_col = rect.col + ratio.primary_units(rect.cols - 1);
                    if col == divider_col && row >= rect.row && row < rect.row + rect.rows {
                        return Some(DividerHit {
                            axis: *axis,
                            path: path.clone(),
                        });
                    }
                }
                SplitAxis::Horizontal if rect.rows >= 3 => {
                    let divider_row = rect.row + ratio.primary_units(rect.rows - 1);
                    if row == divider_row && col >= rect.col && col < rect.col + rect.cols {
                        return Some(DividerHit {
                            axis: *axis,
                            path: path.clone(),
                        });
                    }
                }
                _ => {}
            }

            let (primary_rect, secondary_rect) = split_rect(rect, *axis, *ratio);
            path.push(SplitBranch::Primary);
            if let Some(hit) = divider_hit_at_rect(primary, primary_rect, col, row, path) {
                path.pop();
                return Some(hit);
            }
            path.pop();

            path.push(SplitBranch::Secondary);
            let hit = divider_hit_at_rect(secondary, secondary_rect, col, row, path);
            path.pop();
            hit
        }
    }
}

fn set_split_ratio_at_rect(
    node: &mut Layout,
    rect: CellRect,
    path: &[SplitBranch],
    col: usize,
    row: usize,
    min_leaf_cols: usize,
    min_leaf_rows: usize,
) -> bool {
    match node {
        Layout::Pane(_) => false,
        Layout::Split {
            axis,
            ratio,
            primary,
            secondary,
        } => {
            if path.is_empty() {
                let (primary_min_cols, primary_min_rows) =
                    primary.minimum_size(min_leaf_cols, min_leaf_rows);
                let (secondary_min_cols, secondary_min_rows) =
                    secondary.minimum_size(min_leaf_cols, min_leaf_rows);
                let Some(new_ratio) = ratio_from_cell(
                    *axis,
                    rect,
                    col,
                    row,
                    match axis {
                        SplitAxis::Vertical => primary_min_cols,
                        SplitAxis::Horizontal => primary_min_rows,
                    },
                    match axis {
                        SplitAxis::Vertical => secondary_min_cols,
                        SplitAxis::Horizontal => secondary_min_rows,
                    },
                ) else {
                    return false;
                };
                *ratio = new_ratio;
                return true;
            }
            let (primary_rect, secondary_rect) = split_rect(rect, *axis, *ratio);
            match path[0] {
                SplitBranch::Primary => set_split_ratio_at_rect(
                    primary,
                    primary_rect,
                    &path[1..],
                    col,
                    row,
                    min_leaf_cols,
                    min_leaf_rows,
                ),
                SplitBranch::Secondary => set_split_ratio_at_rect(
                    secondary,
                    secondary_rect,
                    &path[1..],
                    col,
                    row,
                    min_leaf_cols,
                    min_leaf_rows,
                ),
            }
        }
    }
}

fn ratio_from_cell(
    axis: SplitAxis,
    rect: CellRect,
    col: usize,
    row: usize,
    primary_min: usize,
    secondary_min: usize,
) -> Option<SplitRatio> {
    match axis {
        SplitAxis::Vertical => {
            let divider = usize::from(rect.cols >= 3);
            let content = rect.cols.saturating_sub(divider);
            if content <= primary_min.saturating_add(secondary_min) {
                return None;
            }
            let primary = col
                .saturating_sub(rect.col)
                .clamp(primary_min, content.saturating_sub(secondary_min));
            Some(SplitRatio::from_primary_units(primary, content))
        }
        SplitAxis::Horizontal => {
            let divider = usize::from(rect.rows >= 3);
            let content = rect.rows.saturating_sub(divider);
            if content <= primary_min.saturating_add(secondary_min) {
                return None;
            }
            let primary = row
                .saturating_sub(rect.row)
                .clamp(primary_min, content.saturating_sub(secondary_min));
            Some(SplitRatio::from_primary_units(primary, content))
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

fn collect_horizontal_dividers(node: &Layout, rect: CellRect, out: &mut Vec<HorizontalDivider>) {
    match node {
        Layout::Pane(_) => {}
        Layout::Split {
            axis,
            ratio,
            primary,
            secondary,
        } => {
            if *axis == SplitAxis::Horizontal && rect.rows >= 3 {
                let content = rect.rows - 1;
                let primary_rows = ratio.primary_units(content);
                out.push(HorizontalDivider {
                    col: rect.col,
                    row: rect.row + primary_rows,
                    width: rect.cols,
                });
            }
            let (primary_rect, secondary_rect) = split_rect(rect, *axis, *ratio);
            collect_horizontal_dividers(primary, primary_rect, out);
            collect_horizontal_dividers(secondary, secondary_rect, out);
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
    fn initial_layout_three_panes_uses_vertical_spine() {
        let root = Layout::from_initial_panes(&[PaneId(1), PaneId(2), PaneId(3)]);
        let viewports = compute_viewports(&root, PhysicalSize::new(120, 80), cell());

        assert_eq!(root.pane_order(), vec![PaneId(1), PaneId(2), PaneId(3)]);
        assert_eq!(viewports.len(), 3);
        assert!(viewports[&PaneId(3)].col_offset > viewports[&PaneId(2)].col_offset);
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
    fn close_pane_promotes_sibling() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);

        assert!(root.close_pane(PaneId(1)));
        assert_eq!(root, Layout::Pane(PaneId(2)));
    }

    #[test]
    fn close_pane_finds_nested_target() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);
        assert!(root.split_pane(PaneId(2), SplitAxis::Vertical, PaneId(3)));

        assert!(root.close_pane(PaneId(2)));
        let viewports = compute_viewports(&root, PhysicalSize::new(120, 80), cell());

        assert_eq!(viewports.len(), 2);
        assert!(viewports.contains_key(&PaneId(1)));
        assert!(viewports.contains_key(&PaneId(3)));
    }

    #[test]
    fn close_pane_rejects_last_or_missing_pane() {
        let mut root = Layout::Pane(PaneId(1));

        assert!(!root.close_pane(PaneId(1)));
        assert!(!root.close_pane(PaneId(99)));
        assert_eq!(root, Layout::Pane(PaneId(1)));
    }

    #[test]
    fn pane_order_uses_depth_first_primary_then_secondary() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);
        assert!(root.split_pane(PaneId(2), SplitAxis::Horizontal, PaneId(3)));

        assert_eq!(root.pane_order(), vec![PaneId(1), PaneId(2), PaneId(3)]);
    }

    #[test]
    fn adjust_split_for_primary_pane_grows_primary_ratio() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);

        assert!(root.adjust_split_for_pane(
            PaneId(1),
            SplitAxis::Vertical,
            RatioDirection::GrowActive
        ));

        let viewports = compute_viewports(&root, PhysicalSize::new(200, 80), cell());
        assert_eq!(viewports[&PaneId(1)].cols, 11);
        assert_eq!(viewports[&PaneId(2)].cols, 8);
    }

    #[test]
    fn adjust_split_for_secondary_pane_grows_secondary_space() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);

        assert!(root.adjust_split_for_pane(
            PaneId(2),
            SplitAxis::Vertical,
            RatioDirection::GrowActive
        ));

        let viewports = compute_viewports(&root, PhysicalSize::new(200, 80), cell());
        assert_eq!(viewports[&PaneId(1)].cols, 9);
        assert_eq!(viewports[&PaneId(2)].cols, 10);
    }

    #[test]
    fn adjust_split_ignores_axis_without_matching_split() {
        let mut root = Layout::from_initial_panes(&[PaneId(1), PaneId(2)]);

        assert!(!root.adjust_split_for_pane(
            PaneId(1),
            SplitAxis::Horizontal,
            RatioDirection::GrowActive
        ));
    }

    #[test]
    fn single_pane_uses_full_window() {
        let viewports = compute_viewports(&pane(0), PhysicalSize::new(100, 80), cell());
        let vp = viewports[&PaneId(0)];

        assert_eq!(vp.cols, 10);
        assert_eq!(vp.rows, 4);
        assert_eq!(vp.col_offset, 0);
        assert_eq!(vp.row_offset, 0);
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
        assert_eq!(viewports[&PaneId(0)].row_offset, 0);
        assert_eq!(viewports[&PaneId(0)].y_px, 0);
        assert_eq!(viewports[&PaneId(1)].rows, 2);
        assert_eq!(viewports[&PaneId(1)].row_offset, 3);
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
        assert!(viewports[&PaneId(2)].row_offset > viewports[&PaneId(1)].row_offset);
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

    #[test]
    fn horizontal_dividers_include_nested_horizontal_split() {
        let root = Layout::Split {
            axis: SplitAxis::Horizontal,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(Layout::Split {
                axis: SplitAxis::Horizontal,
                ratio: SplitRatio::half(),
                primary: Box::new(pane(1)),
                secondary: Box::new(pane(2)),
            }),
        };
        let dividers = horizontal_dividers(&root, PhysicalSize::new(120, 220), cell());

        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0].row, 5);
        assert!(dividers[1].row > dividers[0].row);
    }

    #[test]
    fn divider_hit_and_drag_updates_vertical_split_ratio() {
        let mut root = Layout::from_initial_panes(&[PaneId(0), PaneId(1)]);
        let hit = divider_hit_at(&root, PhysicalSize::new(120, 80), cell(), 6, 0)
            .expect("vertical divider hit");

        assert!(set_split_ratio_at_cell(
            &mut root,
            &hit,
            PhysicalSize::new(120, 80),
            cell(),
            3,
            0,
            1,
            1,
        ));

        let viewports = compute_viewports(&root, PhysicalSize::new(120, 80), cell());
        assert_eq!(viewports[&PaneId(0)].cols, 3);
        assert_eq!(viewports[&PaneId(1)].col_offset, 4);
    }

    #[test]
    fn divider_hit_and_drag_updates_horizontal_split_ratio() {
        let mut root = Layout::Split {
            axis: SplitAxis::Horizontal,
            ratio: SplitRatio::half(),
            primary: Box::new(pane(0)),
            secondary: Box::new(pane(1)),
        };
        let hit = divider_hit_at(&root, PhysicalSize::new(120, 120), cell(), 0, 2)
            .expect("horizontal divider hit");

        assert!(set_split_ratio_at_cell(
            &mut root,
            &hit,
            PhysicalSize::new(120, 120),
            cell(),
            0,
            3,
            1,
            1,
        ));

        let viewports = compute_viewports(&root, PhysicalSize::new(120, 120), cell());
        assert_eq!(viewports[&PaneId(0)].rows, 3);
        assert_eq!(viewports[&PaneId(1)].row_offset, 4);
    }

    #[test]
    fn divider_drag_rejects_when_minimums_exceed_available_space() {
        let mut root = Layout::from_initial_panes(&[PaneId(0), PaneId(1)]);
        let hit = divider_hit_at(&root, PhysicalSize::new(120, 80), cell(), 6, 0)
            .expect("vertical divider hit");
        let before = root.clone();

        assert!(!set_split_ratio_at_cell(
            &mut root,
            &hit,
            PhysicalSize::new(120, 80),
            cell(),
            3,
            0,
            6,
            1,
        ));
        assert_eq!(root, before);
    }
}
