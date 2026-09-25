pub const MAX_MARKERS: usize = 22;

pub fn marker_count(row_count: usize) -> usize {
    if row_count < 2 {
        0
    } else {
        row_count.min(MAX_MARKERS)
    }
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
        assert_eq!(row_to_marker(49, row_count), Some(10));
        assert_eq!(marker_to_row(11, row_count), Some(52));
        assert_eq!(active_marker(49, row_count), Some(10));
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
