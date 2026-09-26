pub const MAX_MARKERS: usize = 14;
pub const MARKER_HIT_TARGET_WIDTH: f32 = 30.;
pub const MARKER_HIT_TARGET_HEIGHT: f32 = 16.;
pub const MIN_MARKER_HIT_TARGET_HEIGHT: f32 = 12.;
pub const RAIL_SCROLLBAR_INSET: f32 = 20.;
pub const RAIL_VERTICAL_INSET: f32 = 8.;
pub const RAIL_CONTENT_GAP: f32 = 3.;
pub const RAIL_CONTENT_INSET: f32 =
    MARKER_HIT_TARGET_WIDTH + RAIL_SCROLLBAR_INSET + RAIL_CONTENT_GAP;

pub fn marker_slot_height(viewport_height: f32, markers: usize) -> f32 {
    if markers == 0 {
        return 0.;
    }
    ((viewport_height - 2. * RAIL_VERTICAL_INSET).max(0.) / markers as f32)
        .min(MARKER_HIT_TARGET_HEIGHT)
}

pub fn marker_count(row_count: usize) -> usize {
    if row_count < 2 {
        0
    } else {
        row_count.min(MAX_MARKERS)
    }
}

pub fn marker_count_for_height(row_count: usize, viewport_height: f32) -> usize {
    let available_height = (viewport_height - 2. * RAIL_VERTICAL_INSET).max(0.);
    let height_capacity = (available_height / MIN_MARKER_HIT_TARGET_HEIGHT).floor() as usize;
    if height_capacity < 2 {
        return 0;
    }
    row_count.min(MAX_MARKERS).min(height_capacity)
}

pub fn marker_slot_height_for_count(viewport_height: f32, markers: usize) -> f32 {
    if markers == 0 {
        return 0.;
    }
    ((viewport_height - 2. * RAIL_VERTICAL_INSET).max(0.) / markers as f32)
        .min(MARKER_HIT_TARGET_HEIGHT)
}

pub fn row_to_marker(row: usize, row_count: usize) -> Option<usize> {
    let count = marker_count(row_count);
    if count == 0 || row >= row_count {
        return None;
    }
    if count == row_count {
        return Some(row);
    }
    Some(scale_index(row, row_count, count))
}

pub fn marker_to_row(marker: usize, row_count: usize) -> Option<usize> {
    let count = marker_count(row_count);
    if count == 0 || marker >= count {
        return None;
    }
    if count == row_count {
        return Some(marker);
    }
    Some(scale_index(marker, count, row_count))
}

pub fn active_marker(logical_top_row: usize, row_count: usize) -> Option<usize> {
    row_to_marker(logical_top_row.min(row_count.saturating_sub(1)), row_count)
}

pub fn row_to_marker_with_count(row: usize, row_count: usize, markers: usize) -> Option<usize> {
    if markers == 0 || row_count == 0 || row >= row_count {
        return None;
    }
    if markers >= row_count {
        return Some(row);
    }
    Some(scale_index(row, row_count, markers))
}

pub fn marker_to_row_with_count(marker: usize, row_count: usize, markers: usize) -> Option<usize> {
    if markers == 0 || marker >= markers || row_count == 0 {
        return None;
    }
    if markers >= row_count {
        return Some(marker);
    }
    Some(scale_index(marker, markers, row_count))
}

pub fn active_marker_with_count(
    logical_top_row: usize,
    row_count: usize,
    markers: usize,
) -> Option<usize> {
    row_to_marker_with_count(
        logical_top_row.min(row_count.saturating_sub(1)),
        row_count,
        markers,
    )
}

fn scale_index(index: usize, source_count: usize, target_count: usize) -> usize {
    if source_count <= 1 || target_count <= 1 {
        return 0;
    }
    let source_span = (source_count - 1) as u128;
    let target_span = (target_count - 1) as u128;
    ((index as u128 * target_span + source_span / 2) / source_span) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_rail_for_zero_or_one_row() {
        assert_eq!(marker_count(0), 0);
        assert_eq!(marker_count(1), 0);
        assert_eq!(row_to_marker(0, 0), None);
        assert_eq!(marker_to_row(0, 1), None);
    }

    #[test]
    fn maps_short_conversations_one_to_one() {
        assert_eq!(marker_count(4), 4);
        assert_eq!(row_to_marker(0, 4), Some(0));
        assert_eq!(row_to_marker(3, 4), Some(3));
        assert_eq!(row_to_marker(4, 4), None);
        assert_eq!(marker_to_row(2, 4), Some(2));
    }

    #[test]
    fn maps_long_conversations_to_bounded_representatives() {
        let count = marker_count(MAX_MARKERS + 500);
        assert_eq!(count, MAX_MARKERS);
        assert_eq!(row_to_marker(0, MAX_MARKERS + 500), Some(0));
        assert_eq!(
            row_to_marker(MAX_MARKERS + 499, MAX_MARKERS + 500),
            Some(MAX_MARKERS - 1)
        );
        assert_eq!(marker_to_row(0, MAX_MARKERS + 500), Some(0));
        assert_eq!(
            marker_to_row(MAX_MARKERS - 1, MAX_MARKERS + 500),
            Some(MAX_MARKERS + 499)
        );
        assert!(marker_to_row(MAX_MARKERS, MAX_MARKERS + 500).is_none());
    }

    #[test]
    fn maps_middle_rows_to_nearest_representative() {
        let row_count = 100;
        assert_eq!(row_to_marker(49, row_count), Some(6));
        assert_eq!(marker_to_row(11, row_count), Some(84));
        assert_eq!(active_marker(49, row_count), Some(6));
    }

    #[test]
    fn dense_rail_keeps_large_fixed_hit_targets_clear_of_transcript_content() {
        assert_eq!(MAX_MARKERS, 14);
        assert_eq!(marker_count(14), 14);
        assert_eq!(marker_count(15), 14);
        assert_eq!(MARKER_HIT_TARGET_WIDTH, 30.);
        assert_eq!(MARKER_HIT_TARGET_HEIGHT, 16.);
        assert_eq!(MIN_MARKER_HIT_TARGET_HEIGHT, 12.);
        assert_eq!(
            RAIL_CONTENT_INSET,
            MARKER_HIT_TARGET_WIDTH + RAIL_SCROLLBAR_INSET + RAIL_CONTENT_GAP
        );
        // Fourteen 16px targets plus the rail's 8px top/bottom inset fit in
        // a compact 300px transcript viewport without vertical overlap.
        assert!(MAX_MARKERS as f32 * MARKER_HIT_TARGET_HEIGHT + 2. * RAIL_VERTICAL_INSET <= 300.);
    }

    #[test]
    fn flexible_marker_slots_fit_constrained_transcript_heights() {
        assert_eq!(RAIL_SCROLLBAR_INSET, 20.);
        assert_eq!(
            RAIL_CONTENT_INSET,
            MARKER_HIT_TARGET_WIDTH + RAIL_SCROLLBAR_INSET + 3.
        );

        for viewport_height in [120., 180., 300.] {
            let slot = marker_slot_height(viewport_height, MAX_MARKERS);
            assert!(slot > 0.);
            assert!(slot <= MARKER_HIT_TARGET_HEIGHT);
            assert!(
                2. * RAIL_VERTICAL_INSET + MAX_MARKERS as f32 * slot <= viewport_height + 0.01,
                "marker slots overflow a {viewport_height}px viewport"
            );
        }
        assert!(
            (marker_slot_height(120., MAX_MARKERS)
                - (120. - 2. * RAIL_VERTICAL_INSET) / MAX_MARKERS as f32)
                .abs()
                < 0.01
        );
        assert!(
            (marker_slot_height(180., MAX_MARKERS)
                - (180. - 2. * RAIL_VERTICAL_INSET) / MAX_MARKERS as f32)
                .abs()
                < 0.01
        );
        assert_eq!(
            marker_slot_height(300., MAX_MARKERS),
            MARKER_HIT_TARGET_HEIGHT
        );
        assert_eq!(marker_slot_height(120., 0), 0.);
    }

    #[test]
    fn marker_count_adapts_to_measured_height_and_keeps_minimum_target() {
        for (height, expected_markers) in [(120., 8), (180., 13), (300., 14)] {
            let count = marker_count_for_height(100, height);
            assert_eq!(count, expected_markers);
            let slot = marker_slot_height_for_count(height, count);
            assert!(slot >= MIN_MARKER_HIT_TARGET_HEIGHT);
            assert!(slot <= MARKER_HIT_TARGET_HEIGHT);
            assert!(2. * RAIL_VERTICAL_INSET + count as f32 * slot <= height + 0.01);
        }
        assert_eq!(marker_count_for_height(100, 31.), 0);
        assert_eq!(marker_count_for_height(4, 120.), 4);
    }

    #[test]
    fn hides_rail_until_measured_height_fits_two_minimum_targets() {
        for height in [31., 32., 39.] {
            assert_eq!(marker_count_for_height(100, height), 0, "height={height}");
            assert_eq!(active_marker_with_count(50, 100, 0), None);
            assert_eq!(marker_to_row_with_count(0, 100, 0), None);
        }

        assert_eq!(marker_count_for_height(100, 40.), 2);
        assert_eq!(
            marker_slot_height_for_count(40., marker_count_for_height(100, 40.)),
            MIN_MARKER_HIT_TARGET_HEIGHT
        );
    }

    #[test]
    fn height_adaptive_mapping_preserves_endpoints_and_click_targets() {
        let rows = 100;
        let markers = marker_count_for_height(rows, 120.);
        assert_eq!(markers, 8);
        assert_eq!(row_to_marker_with_count(0, rows, markers), Some(0));
        assert_eq!(
            row_to_marker_with_count(rows - 1, rows, markers),
            Some(markers - 1)
        );
        assert_eq!(marker_to_row_with_count(0, rows, markers), Some(0));
        assert_eq!(
            marker_to_row_with_count(markers - 1, rows, markers),
            Some(rows - 1)
        );
        assert_eq!(active_marker_with_count(50, rows, markers), Some(4));
    }

    #[test]
    fn active_marker_tracks_the_logical_top_row() {
        assert_eq!(active_marker(0, 5), Some(0));
        assert_eq!(active_marker(2, 5), Some(2));
        assert_eq!(active_marker(4, 5), Some(4));
        assert_eq!(active_marker(10_000, 5), Some(4));
        assert_eq!(active_marker(0, 1), None);
    }
}
