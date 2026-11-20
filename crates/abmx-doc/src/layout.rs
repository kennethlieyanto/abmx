use serde::{Deserialize, Serialize};

/// One text run as the PDF drew it: a string starting at a point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub text: String,
}

/// A horizontal line of runs, ordered by x.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub y: f32,
    pub runs: Vec<Run>,
}

impl Line {
    pub fn text(&self) -> String {
        let mut out = String::new();
        for run in &self.runs {
            out.push_str(&run.text);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub number: usize,
    pub lines: Vec<Line>,
}

/// A normalized statement: pages in reading order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub pages: Vec<Page>,
}

impl Document {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}
