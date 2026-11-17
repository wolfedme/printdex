//! Keyboard movement of the grid selection.

/// A movement key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// The new selected index after `mv`, in a grid of `len` items laid out in
/// `columns` columns. With nothing selected, any key selects the first item.
pub fn step(selected: Option<usize>, mv: Move, columns: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let cols = columns.max(1);
    let Some(i) = selected.map(|i| i.min(last)) else {
        return Some(0);
    };
    Some(match mv {
        Move::Left => i.saturating_sub(1),
        Move::Right => (i + 1).min(last),
        Move::Up => i.checked_sub(cols).unwrap_or(i),
        Move::Down => {
            if i + cols <= last {
                i + cols
            } else if i / cols < last / cols {
                // The row below is shorter: land on its last item.
                last
            } else {
                i
            }
        }
        Move::Home => 0,
        Move::End => last,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_key_selects_first_item() {
        assert_eq!(step(None, Move::Down, 5, 12), Some(0));
        assert_eq!(step(None, Move::End, 5, 0), None);
    }

    #[test]
    fn right_moves_one_and_wraps_to_next_row() {
        assert_eq!(step(Some(0), Move::Right, 5, 12), Some(1));
        assert_eq!(step(Some(4), Move::Right, 5, 12), Some(5));
        assert_eq!(step(Some(11), Move::Right, 5, 12), Some(11));
    }

    #[test]
    fn left_at_start_stays() {
        assert_eq!(step(Some(0), Move::Left, 5, 12), Some(0));
        assert_eq!(step(Some(5), Move::Left, 5, 12), Some(4));
    }

    #[test]
    fn down_moves_one_row() {
        assert_eq!(step(Some(2), Move::Down, 5, 12), Some(7));
    }

    #[test]
    fn down_into_short_last_row_lands_on_last_item() {
        assert_eq!(step(Some(8), Move::Down, 5, 12), Some(11));
    }

    #[test]
    fn down_on_last_row_clamps() {
        assert_eq!(step(Some(11), Move::Down, 5, 12), Some(11));
        assert_eq!(step(Some(10), Move::Down, 5, 12), Some(10));
    }

    #[test]
    fn up_on_first_row_stays() {
        assert_eq!(step(Some(3), Move::Up, 5, 12), Some(3));
        assert_eq!(step(Some(8), Move::Up, 5, 12), Some(3));
    }

    #[test]
    fn home_end() {
        assert_eq!(step(Some(6), Move::Home, 5, 12), Some(0));
        assert_eq!(step(Some(6), Move::End, 5, 12), Some(11));
    }

    #[test]
    fn stale_selection_is_clamped() {
        assert_eq!(step(Some(40), Move::Left, 5, 12), Some(10));
    }
}
