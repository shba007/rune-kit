use rune_kit_core::{
    InstalledSkill, Lockfile, PluginUpdateStatus, RegistryPluginSummary, RegistrySkillSummary,
};

pub struct Column {
    pub name: &'static str,
    pub width: usize,
    pub flexible: bool,
}

impl Column {
    pub fn fixed(name: &'static str, width: usize) -> Self {
        Self {
            name,
            width,
            flexible: false,
        }
    }

    pub fn flexible(name: &'static str, min_width: usize) -> Self {
        Self {
            name,
            width: min_width,
            flexible: true,
        }
    }
}

pub struct Table {
    budget: usize,
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }

    pub fn column(mut self, column: Column) -> Self {
        self.columns.push(column);
        self
    }

    pub fn add_row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    pub fn render(&self) {
        for row in &self.format_rows() {
            println!("{row}");
        }
    }
}

impl Table {
    pub fn format_rows(&self) -> Vec<String> {
        let sep = " ";
        let fixed_total: usize = self
            .columns
            .iter()
            .filter(|c| !c.flexible)
            .map(|c| c.width)
            .sum::<usize>()
            .saturating_add(sep.len() * self.columns.len().saturating_sub(1));
        let flex_width = self.budget.saturating_sub(fixed_total).max(1);
        let widths: Vec<usize> = self
            .columns
            .iter()
            .map(|c| if c.flexible { flex_width } else { c.width })
            .collect();

        let mut out = Vec::with_capacity(self.rows.len() + 2);
        out.push(
            self.format_row(
                &self
                    .columns
                    .iter()
                    .map(|c| c.name.to_string())
                    .collect::<Vec<_>>(),
                &widths,
            ),
        );
        out.push("-".repeat(fixed_total));
        for row in &self.rows {
            out.push(self.format_row(row, &widths));
        }
        out
    }

    fn format_row(&self, cells: &[String], widths: &[usize]) -> String {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let w = widths[i.min(widths.len() - 1)];
            let text = if self.columns.get(i).is_some_and(|c| c.flexible) {
                truncate_to(cell, w)
            } else {
                cell.clone()
            };
            out.push_str(&format!("{:<w$}", text, w = w));
        }
        out
    }
}

fn truncate_to(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

pub fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
    truncated.push_str("...");
    truncated
}

pub fn render_registry_table(plugins: &[RegistryPluginSummary], lockfile: &Lockfile) -> Table {
    let mut t = Table::new(88)
        .column(Column::fixed("NAME", 20))
        .column(Column::fixed("CURRENT", 10))
        .column(Column::fixed("LATEST", 10))
        .column(Column::fixed("BUILDS", 14))
        .column(Column::flexible("DESCRIPTION", 30));
    for p in plugins {
        let builds = match (p.has_wasm, p.has_native) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        let current = lockfile
            .plugins
            .get(&p.name)
            .map(|e| e.version.as_str())
            .unwrap_or("-");
        t.add_row(vec![
            p.name.clone(),
            current.to_string(),
            p.latest.clone(),
            builds.to_string(),
            p.description.clone().unwrap_or_else(|| "-".to_string()),
        ]);
    }
    t
}

pub fn render_update_status_table(statuses: &[PluginUpdateStatus]) -> Table {
    let mut t = Table::new(70)
        .column(Column::fixed("PLUGIN", 16))
        .column(Column::fixed("CURRENT", 12))
        .column(Column::fixed("LATEST", 12))
        .column(Column::fixed("STATUS", 14))
        .column(Column::flexible("BUILDS", 12));
    for s in statuses {
        let status = if s.has_update {
            "Updateable"
        } else {
            "Up-to-date"
        };
        let builds = match (s.has_wasm, s.has_native) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        t.add_row(vec![
            s.name.clone(),
            s.installed_version.clone(),
            s.latest_version.clone(),
            status.to_string(),
            builds.to_string(),
        ]);
    }
    t
}

pub fn render_list_table(lockfile: &Lockfile) -> Table {
    let mut t = Table::new(84)
        .column(Column::fixed("NAME", 14))
        .column(Column::fixed("VERSION", 10))
        .column(Column::fixed("BUILDS", 8))
        .column(Column::fixed("DESCRIPTION", 34))
        .column(Column::flexible("SOURCE", 14));
    for (name, p) in &lockfile.plugins {
        let builds = match (
            p.binary_path.ends_with(".wasm"),
            p.native_binary_path.is_some(),
        ) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        let desc = p.description.clone().unwrap_or_else(|| "-".to_string());
        let short_desc = truncate_display(&desc, 32);
        t.add_row(vec![
            name.to_string(),
            p.version.clone(),
            builds.to_string(),
            short_desc,
            p.source.clone(),
        ]);
    }
    t
}

/// Minimal shape shared by installed and registry skill rows so a single table
/// renderer serves both the installed view and the remote-registry view.
pub trait SkillRow {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn description(&self) -> Option<&str>;
    fn author(&self) -> Option<&str>;
}

impl SkillRow for InstalledSkill {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }
}

impl SkillRow for RegistrySkillSummary {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        &self.latest
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }
}

/// Skills are content, not code — so the table shows NAME/VERSION/AUTHOR/DESCRIPTION
/// rather than the build columns plugins have.
pub fn render_skills_table<S: SkillRow>(skills: &[S]) -> Table {
    let mut t = Table::new(84)
        .column(Column::fixed("NAME", 18))
        .column(Column::fixed("VERSION", 10))
        .column(Column::fixed("AUTHOR", 18))
        .column(Column::flexible("DESCRIPTION", 30));
    for s in skills {
        let author = s.author().unwrap_or("-").to_string();
        let desc = s.description().unwrap_or("-").to_string();
        let short_desc = truncate_display(&desc, 28);
        t.add_row(vec![
            s.name().to_string(),
            s.version().to_string(),
            author,
            short_desc,
        ]);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flexible_column_gets_leftover_budget() {
        let t = Table::new(100)
            .column(Column::fixed("NAME", 16))
            .column(Column::fixed("LATEST", 10))
            .column(Column::fixed("KIND", 14))
            .column(Column::fixed("STATUS", 12))
            .column(Column::flexible("DESCRIPTION", 40));
        let rows = t.format_rows();
        // 16+10+14+12 = 52 fixed, +4 seps = 56; flex = 44
        assert_eq!(rows[0].len(), 100);
    }

    #[test]
    fn truncation_is_char_aware() {
        assert_eq!(truncate_to("hello world", 8), "hello...");
        assert_eq!(truncate_to("short", 8), "short");
    }

    #[test]
    fn flexible_column_does_not_truncate_when_content_fits() {
        let t = Table::new(100)
            .column(Column::fixed("NAME", 16))
            .column(Column::fixed("LATEST", 10))
            .column(Column::fixed("KIND", 14))
            .column(Column::fixed("STATUS", 12))
            .column(Column::flexible("DESCRIPTION", 40));
        let rows = t.format_rows();
        // "DESCRIPTION" (11 chars) fits in 44-char flexible column without truncation
        let row = &rows[0];
        assert_eq!(row.len(), 100);
        // The flexible column starts at position 56 (after all fixed columns + separators)
        let flex_start = 16 + 1 + 10 + 1 + 14 + 1 + 12 + 1; // 56
        assert_eq!(row[flex_start..].len(), 44);
        assert_eq!(&row[flex_start..flex_start + 11], "DESCRIPTION");
        assert!(
            row[flex_start + 11..flex_start + 44]
                .chars()
                .all(|c| c == ' ')
        );
    }
}
