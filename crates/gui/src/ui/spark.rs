//! CPU / RAM lines drawn with iced's canvas.

use iced::widget::canvas::{self, Frame, Geometry, LineDash, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Theme, mouse};

/// One line: values in 0..=1, oldest first, drawn right-aligned over `slots` positions.
pub struct Line {
    pub values: Vec<f32>,
    pub color: Color,
    pub dashed: bool,
}

pub struct Chart {
    pub lines: Vec<Line>,
    pub slots: usize,
    /// Horizontal guide lines (the chart of the Overview).
    pub grid: Option<Color>,
    pub width: f32,
}

impl<M> canvas::Program<M> for Chart {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        self.paint(&mut frame, bounds);
        vec![frame.into_geometry()]
    }
}

impl Chart {
    /// Every stroke stays inside the bounds: the software renderer masks a stroke that crosses them with the
    /// clip of another canvas, which hides it.
    fn paint(&self, frame: &mut Frame, bounds: Rectangle) {
        let (w, h) = (bounds.width, bounds.height);
        let pad = self.width;
        if let Some(c) = self.grid {
            for f in [0.2, 0.55, 0.9] {
                let y = h * f;
                frame.stroke(
                    &Path::line(Point::new(1.0, y), Point::new(w - 1.0, y)),
                    Stroke::default().with_color(c).with_width(0.6),
                );
            }
        }
        let step = (w - 2.0 * pad) / (self.slots.max(2) - 1) as f32;
        for line in &self.lines {
            let n = line.values.len();
            if n < 2 {
                continue;
            }
            let path = Path::new(|b| {
                for (i, v) in line.values.iter().enumerate() {
                    let x = w - pad - (n - 1 - i) as f32 * step;
                    let y = pad + (h - 2.0 * pad) * (1.0 - v.clamp(0.0, 1.0));
                    if i == 0 {
                        b.move_to(Point::new(x, y));
                    } else {
                        b.line_to(Point::new(x, y));
                    }
                }
            });
            let mut stroke = Stroke::default()
                .with_color(line.color)
                .with_width(self.width);
            if line.dashed {
                stroke.line_dash = LineDash {
                    segments: &[5.0, 3.0],
                    offset: 0,
                };
            }
            frame.stroke(&path, stroke);
        }
    }
}
