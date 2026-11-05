use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser};

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_WIKILINKS
        | Options::ENABLE_GFM
}

pub fn events(text: &str, options: Options) -> Vec<(Event<'_>, Range<usize>)> {
    Parser::new_ext(text, options).into_offset_iter().collect()
}

pub fn parse_flat(text: &str, out: &mut Vec<(u8, u32, u32)>) {
    out.clear();
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        let tag = match event {
            Event::Start(_) => 0,
            Event::End(_) => 1,
            Event::Text(_) => 2,
            Event::Code(_) => 3,
            Event::InlineMath(_) | Event::DisplayMath(_) => 4,
            _ => 5,
        };
        out.push((tag, range.start as u32, range.end as u32));
    }
}
