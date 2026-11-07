//! Which list row stays selected when the rows change under it (#91).

/// The row to select once a list's rows change. `previous` is the identity the selected row had before the change and
/// `old` its position. The row that now has that identity stays selected, wherever it moved. Once nothing has it any
/// more, the position stays, kept within the new list. An empty list gives row 0, which holds nothing.
pub fn reselect<'a, K: PartialEq + ?Sized + 'a>(
    previous: Option<&K>,
    old: usize,
    keys: impl IntoIterator<Item = &'a K>,
) -> usize {
    let mut len = 0;
    for (ix, key) in keys.into_iter().enumerate() {
        if previous == Some(key) {
            return ix;
        }
        len = ix + 1;
    }
    old.min(len.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::reselect;

    #[test]
    fn the_selection_follows_its_row_to_a_new_position() {
        assert_eq!(reselect(Some("c"), 2, ["c", "a", "b"]), 0);
        assert_eq!(reselect(Some("a"), 0, ["b", "c", "a"]), 2);
    }

    #[test]
    fn an_unchanged_order_keeps_the_same_row() {
        assert_eq!(reselect(Some("b"), 1, ["a", "b", "c"]), 1);
    }

    #[test]
    fn a_row_that_left_keeps_the_position_within_the_list() {
        assert_eq!(reselect(Some("b"), 1, ["a", "c", "d"]), 1);
        assert_eq!(reselect(Some("d"), 3, ["a", "b", "c"]), 2, "the last row, not past it");
    }

    #[test]
    fn without_an_identity_the_position_stays_within_the_list() {
        assert_eq!(reselect(None::<&str>, 4, ["a", "b"]), 1);
        assert_eq!(reselect(None::<&str>, 0, ["a", "b"]), 0);
    }

    #[test]
    fn an_empty_list_selects_row_zero() {
        assert_eq!(reselect(Some("a"), 3, Vec::<&str>::new()), 0);
        assert_eq!(reselect(None::<&str>, 0, Vec::<&str>::new()), 0);
    }
}
