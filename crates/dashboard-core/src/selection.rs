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

/// What lies past the rows a paged reload has read so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beyond {
    /// The listing goes on, and the reload can read its next page.
    More,
    /// The listing has ended: there is nothing past these rows.
    End,
    /// The listing goes on, but the reload stopped reading: at its page limit, or on a page that brought nothing or
    /// didn't move the cursor on. Whatever is past these rows is unread, not absent.
    Unread,
}

/// What a paged reload does with the rows it has read so far (#91).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reload {
    /// Show the rows, with this row selected.
    Show(usize),
    /// The selected item isn't among them yet and the listing goes on: read the next page first.
    More,
    /// The selected item isn't among them and the reload stopped before the listing ended. It may only be unread, so
    /// the rows can't stand for the list: keep the rows on screen and their selection, and report the reload as failed.
    Incomplete,
}

/// A paged reload keeps the item that was selected before it (`previous`, at row `old`). Found among the rows read so
/// far, the rows show with it selected, whatever lies `beyond`. Not found, it is gone only once the listing has ended
/// (the rows show with [`reselect`]'s row); while the listing goes on the reload reads on, or, stopped, is incomplete.
/// With no `previous` (new filters, or nothing selected), the rows show at once.
pub fn reload<'a, K: PartialEq + ?Sized + 'a>(
    previous: Option<&K>,
    old: usize,
    keys: impl IntoIterator<Item = &'a K>,
    beyond: Beyond,
) -> Reload {
    let keys: Vec<&K> = keys.into_iter().collect();
    match (previous, beyond) {
        (Some(previous), Beyond::More) if !keys.contains(&previous) => Reload::More,
        (Some(previous), Beyond::Unread) if !keys.contains(&previous) => Reload::Incomplete,
        _ => Reload::Show(reselect(previous, old, keys)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Beyond, Reload, reload, reselect};

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
        assert_eq!(reload(Some("z"), 55, ["a", "b"], Beyond::More), Reload::More);
    }

    #[test]
    fn a_reload_shows_the_rows_once_the_selected_item_turns_up() {
        for beyond in [Beyond::More, Beyond::End, Beyond::Unread] {
            assert_eq!(reload(Some("b"), 55, ["a", "b"], beyond), Reload::Show(1), "{beyond:?}");
        }
    }

    #[test]
    fn a_listing_that_ended_without_the_item_keeps_the_position() {
        assert_eq!(reload(Some("z"), 1, ["a", "b", "c"], Beyond::End), Reload::Show(1));
        assert_eq!(reload(Some("z"), 55, ["a", "b", "c"], Beyond::End), Reload::Show(2));
        assert_eq!(reload(Some("z"), 3, Vec::<&str>::new(), Beyond::End), Reload::Show(0));
    }

    #[test]
    fn a_reload_that_stopped_before_the_end_without_the_item_is_incomplete() {
        assert_eq!(reload(Some("z"), 55, ["a", "b"], Beyond::Unread), Reload::Incomplete);
        assert_eq!(
            reload(Some("z"), 0, Vec::<&str>::new(), Beyond::Unread),
            Reload::Incomplete
        );
    }

    #[test]
    fn without_a_selected_item_the_first_rows_show_at_once() {
        for beyond in [Beyond::More, Beyond::End, Beyond::Unread] {
            assert_eq!(reload(None::<&str>, 0, ["a", "b"], beyond), Reload::Show(0));
            assert_eq!(reload(None::<&str>, 7, ["a", "b"], beyond), Reload::Show(1));
        }
    }
}
