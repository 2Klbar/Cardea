//! The keyboard focus layer. iced has no Tab focus on buttons, so each interactive control registers a
//! [`Target`] while the view is built, in screen order; the keyboard moves a focus key along that list.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// The sidebar.
    Side,
    /// The page (and dialogs).
    Main,
}

#[derive(Clone, Debug)]
pub struct Target<M> {
    pub key: String,
    pub group: Group,
    /// 0 for the window, 1 for an open dialog: only the top layer takes the focus.
    pub layer: u8,
    /// What Enter or Space does; `None` for a text field.
    pub on: Option<M>,
    /// The text field to give the keyboard to.
    pub input: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Tab / Shift+Tab: every target of the top layer, wrapping around.
    Next,
    Prev,
    /// Arrows: the targets of the current group, stopping at the ends.
    Down,
    Up,
    /// F6: the first target of the other group.
    Region,
}

/// The targets that can take the focus: those of the top layer.
pub fn reachable<M>(targets: &[Target<M>]) -> Vec<&Target<M>> {
    let top = targets.iter().map(|t| t.layer).max().unwrap_or(0);
    targets.iter().filter(|t| t.layer == top).collect()
}

/// The key that gets the focus after `mv`.
pub fn step<M>(targets: &[Target<M>], current: Option<&str>, mv: Move) -> Option<String> {
    let list = reachable(targets);
    let at = current.and_then(|c| list.iter().position(|t| t.key == c));
    let key = |i: usize| list.get(i).map(|t| t.key.clone());
    match (mv, at) {
        (_, _) if list.is_empty() => None,
        (Move::Next | Move::Down | Move::Region, None) => key(0),
        (Move::Prev | Move::Up, None) => key(list.len() - 1),
        (Move::Next, Some(i)) => key((i + 1) % list.len()),
        (Move::Prev, Some(i)) => key((i + list.len() - 1) % list.len()),
        (Move::Down | Move::Up, Some(i)) => {
            let group = list[i].group;
            let same: Vec<usize> = (0..list.len())
                .filter(|&j| list[j].group == group)
                .collect();
            let p = same
                .iter()
                .position(|&j| j == i)
                .expect("current is in its group");
            let q = if mv == Move::Down {
                (p + 1).min(same.len() - 1)
            } else {
                p.saturating_sub(1)
            };
            key(same[q])
        }
        (Move::Region, Some(i)) => {
            let group = list[i].group;
            list.iter()
                .find(|t| t.group != group)
                .map(|t| t.key.clone())
                .or(key(i))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, group: Group, layer: u8) -> Target<()> {
        Target {
            key: key.into(),
            group,
            layer,
            on: Some(()),
            input: None,
        }
    }

    #[test]
    fn moves() {
        use Group::*;
        let ts = vec![
            t("a", Side, 0),
            t("b", Side, 0),
            t("c", Main, 0),
            t("d", Main, 0),
        ];
        assert_eq!(step(&ts, None, Move::Next).as_deref(), Some("a"));
        assert_eq!(step(&ts, None, Move::Prev).as_deref(), Some("d"));
        assert_eq!(step(&ts, Some("d"), Move::Next).as_deref(), Some("a"));
        assert_eq!(step(&ts, Some("a"), Move::Prev).as_deref(), Some("d"));
        // Arrows stay in the group and stop at its ends.
        assert_eq!(step(&ts, Some("b"), Move::Down).as_deref(), Some("b"));
        assert_eq!(step(&ts, Some("c"), Move::Up).as_deref(), Some("c"));
        assert_eq!(step(&ts, Some("c"), Move::Down).as_deref(), Some("d"));
        assert_eq!(step(&ts, Some("b"), Move::Region).as_deref(), Some("c"));
        assert_eq!(step(&ts, Some("d"), Move::Region).as_deref(), Some("a"));
        // A key that vanished starts over.
        assert_eq!(step(&ts, Some("gone"), Move::Next).as_deref(), Some("a"));
    }

    #[test]
    fn a_dialog_traps_the_focus() {
        use Group::*;
        let ts = vec![t("a", Side, 0), t("x", Main, 1), t("y", Main, 1)];
        assert_eq!(step(&ts, Some("a"), Move::Next).as_deref(), Some("x"));
        assert_eq!(step(&ts, Some("y"), Move::Next).as_deref(), Some("x"));
        assert_eq!(step(&ts, Some("x"), Move::Prev).as_deref(), Some("y"));
    }
}
