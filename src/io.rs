//! Markdown IO — round-trip between the doc model and a markdown string.
//!
//! The writer walks the document and emits the canonical markdown form for
//! each kind; the reader uses
//! pulldown-cmark to build the doc back up.
//!
//! Decorator round-trip is mediated by the schema: each `DecoratorSpec`
//! declares its `to_markdown` serializer, and a registered parser callback
//! (set per-kind via [`MarkdownIo::with_decorator_reader`]) interprets
//! the textual form on the read path.

use std::collections::HashMap;
use std::rc::Rc;

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::attrs::Attrs;
use crate::format::FormatBits;
use crate::model::{Doc, Node, NodeKey};
use crate::schema::Schema;
use crate::step::NodeSpec;

/// Hook called during parsing when the writer encounters a text fragment
/// that looks like a decorator. The closure decides whether the fragment is
/// in fact a known decorator and returns its `NodeSpec` when it is. Returning
/// `None` means "leave the text as-is".
pub type DecoratorReader = Rc<dyn Fn(&str) -> Option<Vec<NodeSpec>>>;

/// Markdown serialization failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownError {
    UnknownDecorator(String),
}

impl std::fmt::Display for MarkdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDecorator(kind) => write!(f, "unknown decorator kind: {kind}"),
        }
    }
}

impl std::error::Error for MarkdownError {}

#[derive(Default, Clone)]
pub struct MarkdownIo {
    decorator_readers: Vec<DecoratorReader>,
}

impl MarkdownIo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a parser that may recognize a slice of plain text as a
    /// decorator. Readers are tried in order; the first to return `Some`
    /// wins.
    pub fn with_decorator_reader(mut self, reader: DecoratorReader) -> Self {
        self.decorator_readers.push(reader);
        self
    }

    pub fn from_markdown(&self, src: &str, schema: &Schema) -> Doc {
        let mut doc = Doc::empty();
        let root = doc.root;
        // Strip the seed paragraph the empty doc carries; we'll add blocks
        // as the parser walks.
        if let Some(e) = doc.nodes.get_mut(&root).and_then(Node::as_element_mut) {
            let kids = std::mem::take(&mut e.children);
            for k in kids {
                doc.nodes.remove(&k);
                doc.clear_parent(k);
            }
        }
        let mut builder = DocBuilder::new(
            &mut doc,
            &self.decorator_readers,
            schema.has_decorator("link"),
            schema.has_decorator("image"),
        );
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        opts.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(src, opts);
        for event in parser {
            builder.event(event);
        }
        builder.finish();
        if doc
            .get_element(root)
            .map(|e| e.children.is_empty())
            .unwrap_or(true)
        {
            // Always end with at least one paragraph so the caret has
            // somewhere to live.
            let p = doc.fresh_key();
            doc.nodes.insert(
                p,
                Node::Element(crate::model::ElementNode {
                    key: p,
                    kind: "paragraph".into(),
                    attrs: Attrs::new(),
                    children: Vec::new(),
                }),
            );
            doc.set_parent(p, root);
            if let Some(re) = doc.nodes.get_mut(&root).and_then(Node::as_element_mut) {
                re.children.push(p);
            }
        }
        doc
    }

    pub fn to_markdown(&self, doc: &Doc, schema: &Schema) -> Result<String, MarkdownError> {
        if let Some(kind) = doc.nodes().values().find_map(|node| match node {
            Node::Decorator(decorator) if !schema.has_decorator(&decorator.kind) => {
                Some(decorator.kind.clone())
            }
            _ => None,
        }) {
            return Err(MarkdownError::UnknownDecorator(kind));
        }

        let mut out = String::new();
        let Some(root) = doc.get_element(doc.root_key()) else {
            return Ok(out);
        };
        for (idx, &child) in root.children.iter().enumerate() {
            if idx > 0 {
                out.push_str("\n\n");
            }
            write_block(doc, child, schema, &mut out);
        }
        Ok(out)
    }
}

fn write_block(doc: &Doc, key: NodeKey, schema: &Schema, out: &mut String) {
    let Some(node) = doc.get(key) else {
        return;
    };
    match node {
        Node::Element(e) => match e.kind.as_str() {
            "paragraph" => write_inlines(doc, &e.children, schema, out),
            "heading" => {
                let level = e.attrs.get_int("level").unwrap_or(1).clamp(1, 6) as usize;
                for _ in 0..level {
                    out.push('#');
                }
                out.push(' ');
                write_inlines(doc, &e.children, schema, out);
            }
            "blockquote" => {
                let mut inner = String::new();
                for (i, &child) in e.children.iter().enumerate() {
                    if i > 0 {
                        inner.push_str("\n\n");
                    }
                    write_block(doc, child, schema, &mut inner);
                }
                for line in inner.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
                if out.ends_with('\n') {
                    out.pop();
                }
            }
            "code_block" => {
                let lang = e.attrs.get_str("lang").unwrap_or("");
                let mut inner = String::new();
                write_code_contents(doc, &e.children, schema, &mut inner);
                let fence = "`".repeat(longest_backtick_run(&inner).max(2) + 1);
                out.push_str(&fence);
                out.push_str(lang);
                out.push('\n');
                out.push_str(&inner);
                if !inner.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&fence);
            }
            "bullet_list" | "ordered_list" => {
                let ordered = e.kind == "ordered_list";
                for (i, &child) in e.children.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    if ordered {
                        out.push_str(&format!("{}. ", i + 1));
                    } else {
                        out.push_str("- ");
                    }
                    if let Some(li) = doc.get_element(child) {
                        write_inlines(doc, &li.children, schema, out);
                    }
                }
            }
            "table" => write_table(doc, e, schema, out),
            _ => {
                // Unknown elements fall back to paragraph-style flow.
                write_inlines(doc, &e.children, schema, out);
            }
        },
        _ => {
            // A non-element block (shouldn't happen for valid docs).
            write_inlines(doc, std::slice::from_ref(&key), schema, out);
        }
    }
}

fn write_table(doc: &Doc, table: &crate::model::ElementNode, schema: &Schema, out: &mut String) {
    // Collect rows. Each row's `header` attr distinguishes the head from
    // body rows; markdown allows exactly one header row, so use the first
    // row tagged `header` (or the first row outright) as the header.
    let rows: Vec<&crate::model::ElementNode> = table
        .children
        .iter()
        .filter_map(|&k| doc.get_element(k))
        .filter(|r| r.kind == "table_row")
        .collect();
    if rows.is_empty() {
        return;
    }
    let header_idx = rows
        .iter()
        .position(|r| r.attrs.get_bool("header").unwrap_or(false))
        .unwrap_or(0);
    let col_count = rows.iter().map(|r| r.children.len()).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }
    let aligns: Vec<&str> = table
        .attrs
        .get_str("align")
        .unwrap_or("")
        .split(',')
        .collect();
    let write_row = |row: &crate::model::ElementNode, out: &mut String| {
        out.push('|');
        for col in 0..col_count {
            out.push(' ');
            if let Some(&cell_key) = row.children.get(col)
                && let Some(cell) = doc.get_element(cell_key)
            {
                let mut cell_out = String::new();
                write_inlines(doc, &cell.children, schema, &mut cell_out);
                // Pipes inside a cell must be escaped; newlines flatten
                // to a `<br>` so the body stays on one line — markdown
                // tables can't span multiple lines.
                let escaped = cell_out.replace('\\', "\\\\").replace('|', "\\|");
                let flat: String = escaped
                    .chars()
                    .map(|c| if c == '\n' { ' ' } else { c })
                    .collect();
                out.push_str(&flat);
            }
            out.push(' ');
            out.push('|');
        }
    };
    write_row(rows[header_idx], out);
    out.push('\n');
    out.push('|');
    for col in 0..col_count {
        let marker = match aligns.get(col).copied().unwrap_or("") {
            "left" => ":---",
            "center" => ":---:",
            "right" => "---:",
            _ => "---",
        };
        out.push(' ');
        out.push_str(marker);
        out.push(' ');
        out.push('|');
    }
    for (idx, row) in rows.iter().enumerate() {
        if idx == header_idx {
            continue;
        }
        out.push('\n');
        write_row(row, out);
    }
}

fn write_code_contents(doc: &Doc, children: &[NodeKey], schema: &Schema, out: &mut String) {
    for &key in children {
        match doc.get(key) {
            Some(Node::Text(text)) => out.push_str(&text.text),
            Some(Node::Decorator(decorator)) => {
                if let Some(spec) = schema.decorator(&decorator.kind) {
                    out.push_str(&(spec.to_markdown)(&decorator.attrs));
                }
            }
            Some(Node::Element(element)) => {
                write_code_contents(doc, &element.children, schema, out);
            }
            None => {}
        }
    }
}

fn write_inlines(doc: &Doc, children: &[NodeKey], schema: &Schema, out: &mut String) {
    for &k in children {
        let Some(node) = doc.get(k) else {
            continue;
        };
        match node {
            Node::Text(t) => write_formatted_text(&t.text, t.format, out),
            Node::Decorator(d) => {
                if let Some(spec) = schema.decorator(&d.kind) {
                    out.push_str(&(spec.to_markdown)(&d.attrs));
                }
            }
            Node::Element(e) => {
                // Inline elements (links etc.) — write children recursively.
                write_inlines(doc, &e.children, schema, out);
            }
        }
    }
}

fn write_formatted_text(text: &str, format: FormatBits, out: &mut String) {
    // Order matters: outer-to-inner so the markers nest correctly when
    // multiple bits are set. The canonical order is strike → bold → italic
    // → code.
    let mut prefix = String::new();
    let mut suffix = String::new();
    if format.contains(FormatBits::STRIKE) {
        prefix.push_str("~~");
        suffix.insert_str(0, "~~");
    }
    if format.contains(FormatBits::BOLD) {
        prefix.push_str("**");
        suffix.insert_str(0, "**");
    }
    if format.contains(FormatBits::ITALIC) {
        prefix.push('_');
        suffix.insert(0, '_');
    }
    if format.contains(FormatBits::CODE) {
        let delimiter = "`".repeat(longest_backtick_run(text) + 1);
        let padded = text.starts_with('`')
            || text.ends_with('`')
            || (text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty());
        prefix.push_str(&delimiter);
        suffix.insert_str(0, &delimiter);
        if padded {
            prefix.push(' ');
            suffix.insert(0, ' ');
        }
    }
    out.push_str(&prefix);
    if format.contains(FormatBits::CODE) {
        out.push_str(text);
    } else {
        for ch in text.chars() {
            if ch.is_ascii_punctuation() {
                out.push('\\');
            }
            out.push(ch);
        }
    }
    out.push_str(&suffix);
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}

// -- parser ---------------------------------------------------------------

struct DocBuilder<'a> {
    doc: &'a mut Doc,
    readers: &'a [DecoratorReader],
    stack: Vec<Frame>,
    active_format: FormatBits,
    list_kind: Vec<&'static str>,
    /// Set while inside an `![alt](src)` image: the destination, plus the
    /// alt text accumulated from the inner events. Becomes an `image`
    /// decorator on the closing tag.
    image: Option<ImagePend>,
    /// Set while inside a `[text](href)` link, when the schema registers a
    /// `link` decorator: the destination plus the label accumulated from
    /// inner events. Becomes a `link` decorator on the closing tag.
    link: Option<LinkPend>,
    /// Whether the host schema knows the `link` decorator. When false, link
    /// markdown flows through as plain text (the pre-link behaviour).
    link_enabled: bool,
    /// Whether image markdown may materialize an `image` decorator.
    image_enabled: bool,
}

struct ImagePend {
    src: String,
    alt: String,
}

struct LinkPend {
    href: String,
    text: String,
}

enum Frame {
    /// Open block element that accumulates children.
    Block { key: NodeKey },
    /// Open inline scope contributing format bits.
    Format(FormatBits),
}

impl<'a> DocBuilder<'a> {
    fn new(
        doc: &'a mut Doc,
        readers: &'a [DecoratorReader],
        link_enabled: bool,
        image_enabled: bool,
    ) -> Self {
        Self {
            doc,
            readers,
            stack: Vec::new(),
            active_format: FormatBits::NONE,
            list_kind: Vec::new(),
            image: None,
            link: None,
            link_enabled,
            image_enabled,
        }
    }

    fn append_decorator(&mut self, kind: &str, attrs: Attrs) {
        let key = self.doc.fresh_key();
        self.append_inline(Node::Decorator(crate::model::DecoratorNode {
            key,
            kind: kind.to_string(),
            attrs,
        }));
    }

    fn finish(&mut self) {
        // No-op; left for symmetry / future cleanup.
    }

    fn current_block(&self) -> Option<NodeKey> {
        for frame in self.stack.iter().rev() {
            if let Frame::Block { key } = frame {
                return Some(*key);
            }
        }
        None
    }

    fn push_block(&mut self, kind: &str, attrs: Attrs) {
        let parent = self.current_block().unwrap_or(self.doc.root);
        let key = self.doc.fresh_key();
        self.doc.nodes.insert(
            key,
            Node::Element(crate::model::ElementNode {
                key,
                kind: kind.to_string(),
                attrs,
                children: Vec::new(),
            }),
        );
        self.doc.set_parent(key, parent);
        if let Some(pe) = self
            .doc
            .nodes
            .get_mut(&parent)
            .and_then(Node::as_element_mut)
        {
            pe.children.push(key);
        }
        self.stack.push(Frame::Block { key });
    }

    fn pop_block(&mut self) {
        while let Some(frame) = self.stack.pop() {
            if matches!(frame, Frame::Block { .. }) {
                break;
            }
        }
    }

    fn append_inline(&mut self, node: Node) {
        let parent = match self.current_block() {
            Some(p) => p,
            None => {
                // No active block — create a wrapper paragraph.
                self.push_block("paragraph", Attrs::new());
                self.current_block().unwrap()
            }
        };
        let key = node.key();
        self.doc.nodes.insert(key, node);
        self.doc.set_parent(key, parent);
        if let Some(pe) = self
            .doc
            .nodes
            .get_mut(&parent)
            .and_then(Node::as_element_mut)
        {
            pe.children.push(key);
        }
    }

    fn push_text(&mut self, text: &str) {
        // Decorator readers may carve a text run into a sequence of nodes
        // (decorator + plain remainder). Try each reader on the full text;
        // if one matches, use its output verbatim and stop.
        for reader in self.readers {
            if let Some(specs) = reader(text) {
                for spec in specs {
                    let parent = match self.current_block() {
                        Some(p) => p,
                        None => {
                            self.push_block("paragraph", Attrs::new());
                            self.current_block().unwrap()
                        }
                    };
                    let key = match spec {
                        NodeSpec::Text { text, format } => {
                            let k = self.doc.fresh_key();
                            self.doc.nodes.insert(
                                k,
                                Node::Text(crate::model::TextNode {
                                    key: k,
                                    text,
                                    format: FormatBits(format.0 | self.active_format.0),
                                }),
                            );
                            self.doc.set_parent(k, parent);
                            k
                        }
                        NodeSpec::Decorator { kind, attrs } => {
                            let k = self.doc.fresh_key();
                            self.doc.nodes.insert(
                                k,
                                Node::Decorator(crate::model::DecoratorNode {
                                    key: k,
                                    kind,
                                    attrs,
                                }),
                            );
                            self.doc.set_parent(k, parent);
                            k
                        }
                        NodeSpec::Element {
                            kind,
                            attrs,
                            children: _,
                        } => {
                            // Inline elements aren't expected from readers;
                            // store an empty placeholder for now.
                            let k = self.doc.fresh_key();
                            self.doc.nodes.insert(
                                k,
                                Node::Element(crate::model::ElementNode {
                                    key: k,
                                    kind,
                                    attrs,
                                    children: Vec::new(),
                                }),
                            );
                            self.doc.set_parent(k, parent);
                            k
                        }
                    };
                    if let Some(pe) = self
                        .doc
                        .nodes
                        .get_mut(&parent)
                        .and_then(Node::as_element_mut)
                    {
                        pe.children.push(key);
                    }
                }
                return;
            }
        }
        let key = self.doc.fresh_key();
        self.append_inline(Node::Text(crate::model::TextNode {
            key,
            text: text.to_string(),
            format: self.active_format,
        }));
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => self.push_block("paragraph", Attrs::new()),
                Tag::Heading { level, .. } => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    let attrs = Attrs::new().with("level", n as i64);
                    self.push_block("heading", attrs);
                }
                Tag::BlockQuote(_) => self.push_block("blockquote", Attrs::new()),
                Tag::CodeBlock(info) => {
                    let lang = match info {
                        pulldown_cmark::CodeBlockKind::Fenced(s) => s.to_string(),
                        _ => String::new(),
                    };
                    let mut a = Attrs::new();
                    if !lang.is_empty() {
                        a.insert("lang", lang);
                    }
                    self.push_block("code_block", a);
                }
                Tag::List(start) => {
                    let kind = if start.is_some() {
                        "ordered_list"
                    } else {
                        "bullet_list"
                    };
                    self.list_kind.push(kind);
                    self.push_block(kind, Attrs::new());
                }
                Tag::Item => self.push_block("list_item", Attrs::new()),
                Tag::Table(alignments) => {
                    let s = alignments
                        .iter()
                        .map(|a| match a {
                            Alignment::None => "none",
                            Alignment::Left => "left",
                            Alignment::Center => "center",
                            Alignment::Right => "right",
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    let mut attrs = Attrs::new();
                    if !s.is_empty() {
                        attrs.insert("align", s);
                    }
                    self.push_block("table", attrs);
                }
                Tag::TableHead => {
                    self.push_block("table_row", Attrs::new().with("header", true));
                }
                Tag::TableRow => self.push_block("table_row", Attrs::new()),
                Tag::TableCell => self.push_block("table_cell", Attrs::new()),
                Tag::Emphasis => self.stack.push(Frame::Format(FormatBits::ITALIC)),
                Tag::Strong => self.stack.push(Frame::Format(FormatBits::BOLD)),
                Tag::Strikethrough => self.stack.push(Frame::Format(FormatBits::STRIKE)),
                Tag::Link { dest_url, .. } if self.link_enabled && self.image.is_none() => {
                    // Capture the label and destination and emit a decorator
                    // on close. Unregistered links flow through as text.
                    self.link = Some(LinkPend {
                        href: dest_url.to_string(),
                        text: String::new(),
                    });
                }
                Tag::Image { dest_url, .. } if self.image_enabled => {
                    // Open an image: swallow the inner alt events and emit an
                    // `image` decorator on close. Without a registered image
                    // kind, pulldown-cmark's inner alt text flows through.
                    self.image = Some(ImagePend {
                        src: dest_url.to_string(),
                        alt: String::new(),
                    });
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::Table
                | TagEnd::TableHead
                | TagEnd::TableRow
                | TagEnd::TableCell => self.pop_block(),
                TagEnd::List(_) => {
                    self.pop_block();
                    self.list_kind.pop();
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    if let Some(Frame::Format(_)) = self.stack.last() {
                        self.stack.pop();
                    }
                }
                TagEnd::Image => {
                    if let Some(img) = self.image.take() {
                        let attrs = Attrs::new().with("src", img.src).with("alt", img.alt);
                        self.append_decorator("image", attrs);
                    }
                }
                TagEnd::Link => {
                    if let Some(link) = self.link.take() {
                        let label = if link.text.is_empty() {
                            link.href.clone()
                        } else {
                            link.text
                        };
                        let attrs = Attrs::new().with("href", link.href).with("text", label);
                        self.append_decorator("link", attrs);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(img) = self.image.as_mut() {
                    img.alt.push_str(&t);
                    return;
                }
                if let Some(link) = self.link.as_mut() {
                    link.text.push_str(&t);
                    return;
                }
                let fmt = self.compute_format();
                self.with_active_format(fmt, |this| this.push_text(&t));
            }
            Event::Code(t) => {
                if let Some(img) = self.image.as_mut() {
                    img.alt.push_str(&t);
                    return;
                }
                if let Some(link) = self.link.as_mut() {
                    link.text.push_str(&t);
                    return;
                }
                let fmt = FormatBits(self.compute_format().0 | FormatBits::CODE.0);
                self.with_active_format(fmt, |this| this.push_text(&t));
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(img) = self.image.as_mut() {
                    img.alt.push(' ');
                    return;
                }
                if let Some(link) = self.link.as_mut() {
                    link.text.push(' ');
                    return;
                }
                let fmt = self.compute_format();
                self.with_active_format(fmt, |this| this.push_text("\n"));
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                // Pass HTML through unchanged as plain text. The decorator
                // reader has a shot at picking out `<File ...>` etc.
                let fmt = self.compute_format();
                self.with_active_format(fmt, |this| this.push_text(&h));
            }
            _ => {}
        }
    }

    fn compute_format(&self) -> FormatBits {
        let mut fmt = FormatBits::NONE;
        for frame in &self.stack {
            if let Frame::Format(f) = frame {
                fmt.insert(*f);
            }
        }
        fmt
    }

    fn with_active_format<F: FnOnce(&mut Self)>(&mut self, fmt: FormatBits, f: F) {
        let prev = self.active_format;
        self.active_format = fmt;
        f(self);
        self.active_format = prev;
    }
}

/// Convenience for hosts that only need the round-trip without decorator
/// readers. Suitable when the doc has no custom inline nodes.
pub fn to_markdown(doc: &Doc, schema: &Schema) -> Result<String, MarkdownError> {
    MarkdownIo::new().to_markdown(doc, schema)
}

pub fn from_markdown(src: &str, schema: &Schema) -> Doc {
    MarkdownIo::new().from_markdown(src, schema)
}

type HtmlAttrParser = Rc<dyn Fn(&str) -> Option<Attrs>>;

/// Lookup table for HTML-style decorator parsers (e.g. `<File id="…" />`).
/// Hosts build one of these and wrap it in a [`DecoratorReader`] when
/// constructing the editor.
#[derive(Default, Clone)]
pub struct HtmlTagReader {
    /// Tag name → (decorator kind, attribute parser). The tag name (e.g.
    /// `File`) and the decorator kind it maps to (e.g. `file`) are distinct:
    /// the wire format capitalizes the tag while the schema registers the
    /// decorator under a lowercase kind.
    kinds: HashMap<String, (String, HtmlAttrParser)>,
}

impl HtmlTagReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(mut self, tag: impl Into<String>, kind: impl Into<String>, parse: F) -> Self
    where
        F: Fn(&str) -> Option<Attrs> + 'static,
    {
        self.kinds.insert(tag.into(), (kind.into(), Rc::new(parse)));
        self
    }

    /// Wrap as a [`DecoratorReader`] suitable for [`MarkdownIo::with_decorator_reader`].
    pub fn into_reader(self) -> DecoratorReader {
        let me = Rc::new(self);
        // `scan` returns `None` only when nothing matched; an empty `Some`
        // means a fragment was consumed but yields no node (a lone closing
        // tag), so the caller must not fall back to emitting it as text.
        Rc::new(move |text: &str| -> Option<Vec<NodeSpec>> { scan_html_decorators(text, &me) })
    }
}

fn scan_html_decorators(text: &str, reader: &HtmlTagReader) -> Option<Vec<NodeSpec>> {
    if !text.contains('<') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out: Vec<NodeSpec> = Vec::new();
    let mut i = 0;
    let mut last = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // A closing tag `</Name>` for a registered kind: the parser hands the
        // open and close tags over as separate fragments, so the orphan close
        // would otherwise survive as literal text. Swallow it.
        if bytes.get(i + 1) == Some(&b'/') {
            let cname_start = i + 2;
            let mut cj = cname_start;
            while cj < bytes.len()
                && (bytes[cj].is_ascii_alphanumeric() || bytes[cj] == b'_' || bytes[cj] == b'-')
            {
                cj += 1;
            }
            if cj > cname_start
                && bytes.get(cj) == Some(&b'>')
                && reader.kinds.contains_key(&text[cname_start..cj])
            {
                if i > last {
                    out.push(NodeSpec::Text {
                        text: text[last..i].to_string(),
                        format: FormatBits::NONE,
                    });
                }
                last = cj + 1;
                i = cj + 1;
                continue;
            }
            i += 1;
            continue;
        }
        // Try to find a registered tag at this position.
        let name_start = i + 1;
        let mut j = name_start;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
        {
            j += 1;
        }
        if j == name_start {
            i += 1;
            continue;
        }
        let name = &text[name_start..j];
        let (kind, parser) = match reader.kinds.get(name) {
            Some(entry) => entry,
            None => {
                i += 1;
                continue;
            }
        };
        // Find the end of the open or self-closing tag.
        let mut k = j;
        let mut in_quote: Option<u8> = None;
        while k < bytes.len() {
            match (bytes[k], in_quote) {
                (b'"', None) | (b'\'', None) => in_quote = Some(bytes[k]),
                (b, Some(q)) if b == q => in_quote = None,
                (b'>', None) => break,
                _ => {}
            }
            k += 1;
        }
        if k >= bytes.len() {
            i += 1;
            continue;
        }
        // Slice of attributes: bytes (j..k), stripped of trailing `/`.
        let mut attr_end = k;
        while attr_end > j
            && (bytes[attr_end - 1] == b'/' || bytes[attr_end - 1].is_ascii_whitespace())
        {
            attr_end -= 1;
        }
        let attr_str = &text[j..attr_end];
        let Some(attrs) = parser(attr_str) else {
            i += 1;
            continue;
        };
        // Handle the optional close tag `</Name>` immediately after.
        let mut consumed_end = k + 1;
        let close_tag = format!("</{name}>");
        if text[consumed_end..].starts_with(&close_tag) {
            consumed_end += close_tag.len();
        }
        // Emit plain text before the match, then the decorator.
        if i > last {
            out.push(NodeSpec::Text {
                text: text[last..i].to_string(),
                format: FormatBits::NONE,
            });
        }
        out.push(NodeSpec::Decorator {
            kind: kind.clone(),
            attrs,
        });
        last = consumed_end;
        i = consumed_end;
    }
    if last == 0 {
        return None;
    }
    if last < text.len() {
        out.push(NodeSpec::Text {
            text: text[last..].to_string(),
            format: FormatBits::NONE,
        });
    }
    Some(out)
}

/// Helper for hosts: parse a simple attribute string (`id="x" name="y"`)
/// into [`Attrs`].
pub fn parse_html_attrs(input: &str) -> Attrs {
    let mut out = Attrs::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
        {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name = &input[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            // bool attribute
            out.insert(name.to_string(), true);
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote == b'"' || quote == b'\'' {
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            let val = &input[val_start..i];
            out.insert(name.to_string(), val.to_string());
            if i < bytes.len() {
                i += 1;
            }
        } else {
            let val_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let val = &input[val_start..i];
            out.insert(name.to_string(), val.to_string());
        }
    }
    out
}
