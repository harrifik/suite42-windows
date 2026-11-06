//! Presentation model: slides of shapes (geometry, fill, line, rich text, pictures, tables),
//! backgrounds, notes, transitions and simple animations; themes and layouts.

use kit42::richtext::{Align2, CharFmt, Para, ParaStyle};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geom {
    Rect,
    RoundRect,
    Ellipse,
    Triangle,
    RightTriangle,
    Diamond,
    Pentagon,
    Hexagon,
    RightArrow,
    LeftArrow,
    UpArrow,
    DownArrow,
    Star5,
    Heart,
    Chevron,
    Plus,
    Callout,
    Line,
    Arrow,
}

impl Geom {
    pub const SHAPES: [Geom; 17] = [
        Geom::Rect, Geom::RoundRect, Geom::Ellipse, Geom::Triangle, Geom::RightTriangle, Geom::Diamond, Geom::Pentagon, Geom::Hexagon,
        Geom::RightArrow, Geom::LeftArrow, Geom::UpArrow, Geom::DownArrow, Geom::Star5, Geom::Heart, Geom::Chevron, Geom::Plus, Geom::Callout,
    ];

    pub fn prst(self) -> &'static str {
        match self {
            Geom::Rect => "rect",
            Geom::RoundRect => "roundRect",
            Geom::Ellipse => "ellipse",
            Geom::Triangle => "triangle",
            Geom::RightTriangle => "rtTriangle",
            Geom::Diamond => "diamond",
            Geom::Pentagon => "pentagon",
            Geom::Hexagon => "hexagon",
            Geom::RightArrow => "rightArrow",
            Geom::LeftArrow => "leftArrow",
            Geom::UpArrow => "upArrow",
            Geom::DownArrow => "downArrow",
            Geom::Star5 => "star5",
            Geom::Heart => "heart",
            Geom::Chevron => "chevron",
            Geom::Plus => "plus",
            Geom::Callout => "wedgeRectCallout",
            Geom::Line => "line",
            Geom::Arrow => "straightConnector1",
        }
    }

    pub fn from_prst(s: &str) -> Geom {
        match s {
            "roundRect" | "round2SameRect" | "snipRoundRect" => Geom::RoundRect,
            "ellipse" | "flowChartConnector" | "donut" => Geom::Ellipse,
            "triangle" | "flowChartExtract" => Geom::Triangle,
            "rtTriangle" => Geom::RightTriangle,
            "diamond" | "flowChartDecision" => Geom::Diamond,
            "pentagon" | "homePlate" => Geom::Pentagon,
            "hexagon" => Geom::Hexagon,
            "rightArrow" => Geom::RightArrow,
            "leftArrow" => Geom::LeftArrow,
            "upArrow" => Geom::UpArrow,
            "downArrow" => Geom::DownArrow,
            "star5" | "star4" | "star6" | "star8" => Geom::Star5,
            "heart" => Geom::Heart,
            "chevron" => Geom::Chevron,
            "plus" | "mathPlus" => Geom::Plus,
            "wedgeRectCallout" | "wedgeRoundRectCallout" | "wedgeEllipseCallout" | "cloudCallout" => Geom::Callout,
            "line" | "straightConnector1" | "bentConnector3" => Geom::Line,
            _ => Geom::Rect,
        }
    }

    pub fn name(self) -> &'static str {
        use kit42::t;
        match self {
            Geom::Rect => t("Rectangle", "Прямоугольник"),
            Geom::RoundRect => t("Rounded Rectangle", "Скруглённый прямоугольник"),
            Geom::Ellipse => t("Oval", "Овал"),
            Geom::Triangle => t("Triangle", "Треугольник"),
            Geom::RightTriangle => t("Right Triangle", "Прямоугольный треугольник"),
            Geom::Diamond => t("Diamond", "Ромб"),
            Geom::Pentagon => t("Pentagon", "Пятиугольник"),
            Geom::Hexagon => t("Hexagon", "Шестиугольник"),
            Geom::RightArrow => t("Right Arrow", "Стрелка вправо"),
            Geom::LeftArrow => t("Left Arrow", "Стрелка влево"),
            Geom::UpArrow => t("Up Arrow", "Стрелка вверх"),
            Geom::DownArrow => t("Down Arrow", "Стрелка вниз"),
            Geom::Star5 => t("Star", "Звезда"),
            Geom::Heart => t("Heart", "Сердце"),
            Geom::Chevron => t("Chevron", "Шеврон"),
            Geom::Plus => t("Plus", "Плюс"),
            Geom::Callout => t("Callout", "Выноска"),
            Geom::Line => t("Line", "Линия"),
            Geom::Arrow => t("Arrow", "Стрелка"),
        }
    }

    /// Outline in unit coordinates (0..1), clockwise; None for ellipses.
    pub fn outline(self) -> Option<Vec<(f32, f32)>> {
        Some(match self {
            Geom::Rect | Geom::RoundRect => vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            Geom::Ellipse => return None,
            Geom::Triangle => vec![(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)],
            Geom::RightTriangle => vec![(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            Geom::Diamond => vec![(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)],
            Geom::Pentagon => vec![(0.5, 0.0), (1.0, 0.38), (0.81, 1.0), (0.19, 1.0), (0.0, 0.38)],
            Geom::Hexagon => vec![(0.25, 0.0), (0.75, 0.0), (1.0, 0.5), (0.75, 1.0), (0.25, 1.0), (0.0, 0.5)],
            Geom::RightArrow => vec![(0.0, 0.25), (0.65, 0.25), (0.65, 0.0), (1.0, 0.5), (0.65, 1.0), (0.65, 0.75), (0.0, 0.75)],
            Geom::LeftArrow => vec![(1.0, 0.25), (0.35, 0.25), (0.35, 0.0), (0.0, 0.5), (0.35, 1.0), (0.35, 0.75), (1.0, 0.75)],
            Geom::UpArrow => vec![(0.25, 1.0), (0.25, 0.35), (0.0, 0.35), (0.5, 0.0), (1.0, 0.35), (0.75, 0.35), (0.75, 1.0)],
            Geom::DownArrow => vec![(0.25, 0.0), (0.25, 0.65), (0.0, 0.65), (0.5, 1.0), (1.0, 0.65), (0.75, 0.65), (0.75, 0.0)],
            Geom::Star5 => (0..10)
                .map(|k| {
                    let r = if k % 2 == 0 { 0.5 } else { 0.2 };
                    let a = (k as f32 * 36.0 - 90.0).to_radians();
                    (0.5 + r * a.cos(), 0.52 + r * a.sin())
                })
                .collect(),
            Geom::Heart => (0..40)
                .map(|k| {
                    let tt = k as f32 / 40.0 * std::f32::consts::TAU;
                    let x = 16.0 * tt.sin().powi(3);
                    let y = 13.0 * tt.cos() - 5.0 * (2.0 * tt).cos() - 2.0 * (3.0 * tt).cos() - (4.0 * tt).cos();
                    (0.5 + x / 34.0, 0.45 - y / 34.0)
                })
                .collect(),
            Geom::Chevron => vec![(0.0, 0.0), (0.7, 0.0), (1.0, 0.5), (0.7, 1.0), (0.0, 1.0), (0.3, 0.5)],
            Geom::Plus => vec![(0.35, 0.0), (0.65, 0.0), (0.65, 0.35), (1.0, 0.35), (1.0, 0.65), (0.65, 0.65), (0.65, 1.0), (0.35, 1.0), (0.35, 0.65), (0.0, 0.65), (0.0, 0.35), (0.35, 0.35)],
            Geom::Callout => vec![(0.0, 0.0), (1.0, 0.0), (1.0, 0.75), (0.4, 0.75), (0.2, 1.0), (0.25, 0.75), (0.0, 0.75)],
            Geom::Line | Geom::Arrow => vec![(0.0, 0.0), (1.0, 1.0)],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placeholder {
    Title,
    Subtitle,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anim {
    Appear,
    Fade,
    FlyIn,
    Zoom,
}

impl Anim {
    pub const ALL: [Anim; 4] = [Anim::Appear, Anim::Fade, Anim::FlyIn, Anim::Zoom];
    pub fn name(self) -> &'static str {
        use kit42::t;
        match self {
            Anim::Appear => t("Appear", "Появление"),
            Anim::Fade => t("Fade", "Выцветание"),
            Anim::FlyIn => t("Fly In", "Вылет"),
            Anim::Zoom => t("Zoom", "Масштабирование"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    None,
    Fade,
    Push,
    Wipe,
}

#[derive(Clone, Debug)]
pub enum Content {
    Shape(Geom),
    Picture(Arc<Vec<u8>>),
    Table(Vec<Vec<Para>>),
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub id: u64,
    pub content: Content,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub fill: Option<[u8; 3]>,
    pub line: Option<([u8; 3], f32)>,
    pub text: Vec<Para>,
    pub anchor: Anchor,
    pub placeholder: Option<Placeholder>,
    pub anim: Option<Anim>,
    pub name: String,
}

impl Shape {
    pub fn new(content: Content, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            id: rand::random(),
            content,
            x,
            y,
            w,
            h,
            rot: 0.0,
            flip_h: false,
            flip_v: false,
            fill: None,
            line: None,
            text: vec![],
            anchor: Anchor::Top,
            placeholder: None,
            anim: None,
            name: String::new(),
        }
    }

    pub fn text_box(x: f32, y: f32, w: f32, h: f32, text: &str, fmt: CharFmt) -> Self {
        let mut s = Self::new(Content::Shape(Geom::Rect), x, y, w, h);
        s.text = text.split('\n').map(|l| para(l, fmt.clone())).collect();
        s
    }

    pub fn plain_text(&self) -> String {
        self.text.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n")
    }

    pub fn has_text(&self) -> bool {
        !self.text.is_empty() && self.text.iter().any(|p| !p.is_empty())
    }

    pub fn is_line(&self) -> bool {
        matches!(self.content, Content::Shape(Geom::Line) | Content::Shape(Geom::Arrow))
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

pub fn para(text: &str, fmt: CharFmt) -> Para {
    let mut p = Para::new(text, fmt);
    p.space_after = 4.0;
    p.line_spacing = 1.0;
    p
}

#[derive(Clone, Debug)]
pub enum Background {
    Solid([u8; 3]),
    Gradient([u8; 3], [u8; 3]),
    Picture(Arc<Vec<u8>>),
}

#[derive(Clone, Debug)]
pub struct Slide {
    pub id: u64,
    pub shapes: Vec<Shape>,
    pub background: Option<Background>,
    pub notes: String,
    pub hidden: bool,
    pub transition: Transition,
    pub layout: String,
}

impl Slide {
    pub fn new() -> Self {
        Self { id: rand::random(), shapes: vec![], background: None, notes: String::new(), hidden: false, transition: Transition::None, layout: "Blank".into() }
    }

    pub fn title(&self) -> String {
        self.shapes.iter().find(|s| s.placeholder == Some(Placeholder::Title)).map(|s| s.plain_text()).filter(|t| !t.trim().is_empty()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub title: [u8; 3],
    pub accents: [[u8; 3]; 6],
    pub major: String,
    pub minor: String,
}

pub fn themes() -> Vec<Theme> {
    vec![
        Theme { name: "Office", background: [255, 255, 255], text: [38, 38, 38], title: [38, 38, 38], accents: [[68, 114, 196], [237, 125, 49], [165, 165, 165], [255, 192, 0], [91, 155, 213], [112, 173, 71]], major: "Calibri Light".into(), minor: "Calibri".into() },
        Theme { name: "Midnight", background: [30, 34, 45], text: [230, 232, 238], title: [255, 255, 255], accents: [[90, 160, 255], [255, 140, 90], [140, 220, 160], [255, 210, 90], [190, 140, 255], [120, 200, 220]], major: "Segoe UI Light".into(), minor: "Segoe UI".into() },
        Theme { name: "Ocean", background: [235, 244, 250], text: [20, 50, 70], title: [0, 84, 140], accents: [[0, 120, 190], [0, 170, 160], [250, 160, 40], [230, 80, 80], [120, 90, 200], [90, 180, 80]], major: "Georgia".into(), minor: "Segoe UI".into() },
        Theme { name: "Forest", background: [244, 247, 240], text: [40, 55, 35], title: [46, 104, 52], accents: [[70, 130, 60], [200, 150, 60], [120, 90, 60], [90, 160, 160], [180, 80, 60], [150, 150, 80]], major: "Cambria".into(), minor: "Calibri".into() },
        Theme { name: "Warm", background: [255, 248, 240], text: [70, 45, 30], title: [190, 80, 40], accents: [[220, 100, 50], [240, 170, 60], [170, 70, 90], [100, 140, 90], [80, 110, 160], [200, 160, 120]], major: "Constantia".into(), minor: "Corbel".into() },
        Theme { name: "Minimal", background: [250, 250, 250], text: [60, 60, 60], title: [20, 20, 20], accents: [[40, 40, 40], [130, 130, 130], [200, 60, 60], [60, 120, 200], [60, 160, 120], [220, 170, 40]], major: "Segoe UI Semibold".into(), minor: "Segoe UI".into() },
    ]
}

#[derive(Clone, Debug)]
pub struct Presentation {
    pub width: f32,
    pub height: f32,
    pub slides: Vec<Slide>,
    pub theme: Theme,
    pub title: String,
    pub author: String,
}

impl Default for Presentation {
    fn default() -> Self {
        let mut p = Self { width: 960.0, height: 540.0, slides: vec![], theme: themes()[0].clone(), title: String::new(), author: String::new() };
        p.slides.push(p.layout_slide(0));
        p
    }
}

pub const LAYOUTS: [&str; 6] = ["Title Slide", "Title and Content", "Section Header", "Two Content", "Title Only", "Blank"];

pub fn layout_name(i: usize) -> &'static str {
    use kit42::t;
    [
        t("Title Slide", "Титульный слайд"),
        t("Title and Content", "Заголовок и объект"),
        t("Section Header", "Заголовок раздела"),
        t("Two Content", "Два объекта"),
        t("Title Only", "Только заголовок"),
        t("Blank", "Пустой слайд"),
    ][i]
}

impl Presentation {
    pub fn fmt(&self, size: f32, title: bool) -> CharFmt {
        CharFmt { font: if title { self.theme.major.clone() } else { self.theme.minor.clone() }, size, color: if title { self.theme.title } else { self.theme.text }, ..CharFmt::default() }
    }

    fn ph(&self, kind: Placeholder, x: f32, y: f32, w: f32, h: f32, hint: &str, size: f32) -> Shape {
        let mut s = Shape::new(Content::Shape(Geom::Rect), x, y, w, h);
        s.placeholder = Some(kind);
        s.name = hint.to_string();
        let mut p = para("", self.fmt(size, kind == Placeholder::Title));
        if kind != Placeholder::Body {
            p.align = Align2::Center;
        }
        if kind == Placeholder::Body {
            p.list = Some(kit42::richtext::ListInfo { kind: kit42::richtext::ListKind::Bullet, level: 0, checked: false });
        }
        s.text = vec![p];
        s.anchor = if kind == Placeholder::Title { Anchor::Middle } else { Anchor::Top };
        s
    }

    /// A new slide with placeholders for a layout.
    pub fn layout_slide(&self, layout: usize) -> Slide {
        use kit42::t;
        let (w, h) = (self.width, self.height);
        let m = w * 0.06;
        let mut s = Slide::new();
        s.layout = LAYOUTS[layout].into();
        let title_hint = t("Click to add title", "Заголовок слайда");
        let body_hint = t("Click to add text", "Текст слайда");
        match layout {
            0 => {
                let mut tt = self.ph(Placeholder::Title, m, h * 0.24, w - 2.0 * m, h * 0.3, title_hint, 54.0);
                tt.anchor = Anchor::Bottom;
                s.shapes.push(tt);
                s.shapes.push(self.ph(Placeholder::Subtitle, m * 2.0, h * 0.57, w - 4.0 * m, h * 0.18, t("Click to add subtitle", "Подзаголовок"), 24.0));
            }
            1 => {
                s.shapes.push(self.ph(Placeholder::Title, m, h * 0.05, w - 2.0 * m, h * 0.18, title_hint, 40.0));
                s.shapes.push(self.ph(Placeholder::Body, m, h * 0.26, w - 2.0 * m, h * 0.64, body_hint, 24.0));
            }
            2 => {
                let mut tt = self.ph(Placeholder::Title, m, h * 0.3, w - 2.0 * m, h * 0.3, title_hint, 48.0);
                tt.text[0].align = Align2::Left;
                tt.anchor = Anchor::Bottom;
                s.shapes.push(tt);
                let mut st = self.ph(Placeholder::Subtitle, m, h * 0.62, w - 2.0 * m, h * 0.14, body_hint, 22.0);
                st.text[0].align = Align2::Left;
                s.shapes.push(st);
            }
            3 => {
                s.shapes.push(self.ph(Placeholder::Title, m, h * 0.05, w - 2.0 * m, h * 0.18, title_hint, 40.0));
                let cw = (w - 3.0 * m) / 2.0;
                s.shapes.push(self.ph(Placeholder::Body, m, h * 0.26, cw, h * 0.64, body_hint, 22.0));
                s.shapes.push(self.ph(Placeholder::Body, 2.0 * m + cw, h * 0.26, cw, h * 0.64, body_hint, 22.0));
            }
            4 => {
                s.shapes.push(self.ph(Placeholder::Title, m, h * 0.05, w - 2.0 * m, h * 0.18, title_hint, 40.0));
            }
            _ => {}
        }
        s
    }

    pub fn background_of(&self, s: &Slide) -> Background {
        s.background.clone().unwrap_or(Background::Solid(self.theme.background))
    }
}

/// Plain paragraphs for an outline export.
pub fn outline_md(p: &Presentation) -> String {
    let mut out = String::new();
    for (i, s) in p.slides.iter().enumerate() {
        let title = s.title();
        out.push_str(&format!("## {}. {}\n\n", i + 1, if title.is_empty() { kit42::t("Untitled", "Без заголовка") } else { title.as_str() }));
        for sh in s.shapes.iter().filter(|x| x.placeholder != Some(Placeholder::Title)) {
            for p in &sh.text {
                let t = p.text();
                if t.trim().is_empty() {
                    continue;
                }
                let indent = "  ".repeat(p.list.as_ref().map(|l| l.level as usize).unwrap_or(0));
                out.push_str(&format!("{indent}- {t}\n"));
            }
            if let Content::Table(rows) = &sh.content {
                for r in rows {
                    out.push_str(&format!("| {} |\n", r.iter().map(|c| c.text()).collect::<Vec<_>>().join(" | ")));
                }
            }
        }
        if !s.notes.trim().is_empty() {
            out.push_str(&format!("\n> {}\n", s.notes.replace('\n', "\n> ")));
        }
        out.push('\n');
    }
    out
}

pub fn is_title(style: ParaStyle) -> bool {
    matches!(style, ParaStyle::Title | ParaStyle::Heading(_))
}
