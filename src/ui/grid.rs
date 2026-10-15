//! Where each entry sits in a view laid out in lines of cells: the rows of
//! the Icons view, the columns of the List view. Folders fill the first
//! lines and the files start on a line of their own, like File Pilot.

/// The lines' shape: `per_line` cells each, `folders` first, `total` cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridShape {
    pub per_line: usize,
    pub folders: usize,
    pub total: usize,
}

impl GridShape {
    pub fn new(per_line: usize, folders: usize, total: usize) -> Self {
        Self { per_line: per_line.max(1), folders: folders.min(total), total }
    }

    /// Lines the folders take.
    fn folder_lines(&self) -> usize {
        self.folders.div_ceil(self.per_line)
    }

    pub fn lines(&self) -> usize {
        self.folder_lines() + (self.total - self.folders).div_ceil(self.per_line)
    }

    /// Cell `i`'s line and place along it.
    pub fn cell(&self, i: usize) -> (usize, usize) {
        if i < self.folders {
            (i / self.per_line, i % self.per_line)
        } else {
            let file = i - self.folders;
            (self.folder_lines() + file / self.per_line, file % self.per_line)
        }
    }

    /// The cells on `line`, as a range of entries.
    pub fn line(&self, line: usize) -> std::ops::Range<usize> {
        let (start, end) = if line < self.folder_lines() {
            (line * self.per_line, self.folders)
        } else {
            (self.folders + (line - self.folder_lines()) * self.per_line, self.total)
        };
        start.min(self.total)..(start + self.per_line).min(end)
    }

    /// From cell `i`, `lines` lines over (negative: back) at the same place,
    /// or the line's last cell when it is shorter; held to the first and last line.
    pub fn across(&self, i: usize, lines: isize) -> usize {
        if self.total == 0 {
            return 0;
        }
        let (line, place) = self.cell(i.min(self.total - 1));
        let target = line.saturating_add_signed(lines).min(self.lines() - 1);
        let cells = self.line(target);
        (cells.start + place).min(cells.end - 1)
    }

    /// From cell `i`, `cells` cells along the order, held to the ends.
    pub fn along(&self, i: usize, cells: isize) -> usize {
        i.saturating_add_signed(cells).min(self.total.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_fill_their_own_lines_and_files_start_a_new_one() {
        // 5 folders and 4 files, 3 per line: F F F / F F / f f f / f
        let g = GridShape::new(3, 5, 9);
        assert_eq!(g.lines(), 4);
        let cells: Vec<(usize, usize)> = (0..9).map(|i| g.cell(i)).collect();
        assert_eq!(cells, [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (2, 0), (2, 1), (2, 2), (3, 0)]);
        assert_eq!([g.line(0), g.line(1), g.line(2), g.line(3)], [0..3, 3..5, 5..8, 8..9]);
        assert_eq!(GridShape::new(3, 0, 4).lines(), 2, "no folders: no folder line");
        assert_eq!(GridShape::new(0, 1, 2).per_line, 1, "never zero cells per line");
    }

    #[test]
    fn across_keeps_the_place_or_takes_a_shorter_lines_last_cell_and_stops_at_the_ends() {
        let g = GridShape::new(3, 5, 9);
        assert_eq!(g.across(2, 1), 4, "under the third folder: the short line's last");
        assert_eq!(g.across(4, 1), 6, "folder line to the files' line, same place");
        assert_eq!(g.across(7, 1), 8, "the last line holds one file");
        assert_eq!(g.across(8, 1), 8, "already on the last line");
        assert_eq!(g.across(6, -2), 1);
        assert_eq!(g.across(1, -1), 1, "already on the first line");
        assert_eq!(g.across(0, 99), 8, "a page past the end: the last line");
        assert_eq!(GridShape::new(3, 0, 0).across(0, 1), 0, "empty folder");
    }

    #[test]
    fn along_walks_the_order_held_to_the_ends() {
        let g = GridShape::new(3, 5, 9);
        assert_eq!(g.along(4, 1), 5, "the last folder to the first file");
        assert_eq!(g.along(0, -1), 0);
        assert_eq!(g.along(8, 1), 8);
    }
}
