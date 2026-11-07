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

/// What a paged reload does with the rows it has read so far (#91).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reload {
    /// Show the rows, with this row selected.
    Show(usize),
    /// The selected item isn't among them yet and the listing goes on: read the next page first.
    More,
}

/// A paged reload keeps the item that was selected before it (`previous`, at row `old`). While that item isn't among
/// the rows read so far and `more` says the listing goes on, it isn't gone, only not read yet: read on. Once it is
/// found, or the listing has ended without it, the rows show with [`reselect`]'s row. With no `previous` (new filters,
/// or nothing selected), the first rows show at once.
pub fn reload<'a, K: PartialEq + ?Sized + 'a>(
    previous: Option<&K>,
    old: usize,
    keys: impl IntoIterator<Item = &'a K>,
    more: bool,
) -> Reload {
    let keys: Vec<&K> = keys.into_iter().collect();
    match previous {
        Some(previous) if more && !keys.contains(&previous) => Reload::More,
        _ => Reload::Show(reselect(previous, old, keys)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Reload, reload, reselect};

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

    #[test]
    fn a_reload_reads_on_while_the_selected_item_is_only_not_read_yet() {
        assert_eq!(reload(Some("z"), 55, ["a", "b"], true), Reload::More);
    }

    #[test]
    fn a_reload_shows_the_rows_once_the_selected_item_turns_up() {
        assert_eq!(reload(Some("b"), 55, ["a", "b"], true), Reload::Show(1));
        assert_eq!(reload(Some("b"), 55, ["a", "b"], false), Reload::Show(1));
    }

    #[test]
    fn a_listing_that_ended_without_the_item_keeps_the_position() {
        assert_eq!(reload(Some("z"), 1, ["a", "b", "c"], false), Reload::Show(1));
        assert_eq!(reload(Some("z"), 55, ["a", "b", "c"], false), Reload::Show(2));
        assert_eq!(reload(Some("z"), 3, Vec::<&str>::new(), false), Reload::Show(0));
    }

    #[test]
    fn without_a_selected_item_the_first_rows_show_at_once() {
        assert_eq!(reload(None::<&str>, 0, ["a", "b"], true), Reload::Show(0));
        assert_eq!(reload(None::<&str>, 7, ["a", "b"], true), Reload::Show(1));
    }
}
