use super::*;

#[test]
fn dumps_a_small_document() {
    let tree = MarkdownTree::parse("# Hi *x*\n");
    assert_eq!(
        tree.dump(),
        "Document 0..9\n  ATXHeading1 0..8\n    HeaderMark 0..1\n    Emphasis 5..8\n      EmphasisMark 5..6\n      EmphasisMark 7..8\n"
    );
}

#[test]
fn navigates_parent_children_siblings_and_markers() {
    let text = "# Title\n\n- one **two**\n- three\n";
    let tree = MarkdownTree::parse(text);
    let root = tree.root();
    assert_eq!(root.kind(), NodeKind::Document);
    assert_eq!(root.range(), 0..text.len());
    let kinds: Vec<_> = root.children().map(|n| n.kind()).collect();
    assert_eq!(kinds, [NodeKind::AtxHeading(1), NodeKind::BulletList]);
    let heading = root.first_child().unwrap();
    assert_eq!(heading.child(NodeKind::HeaderMark).unwrap().range(), 0..1);
    assert_eq!(heading.next_sibling().unwrap().kind(), NodeKind::BulletList);
    assert!(heading.prev_sibling().is_none());
    let list = root.last_child().unwrap();
    let items: Vec<_> = list.children().collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].prev_sibling().unwrap().range(), items[0].range());
    assert_eq!(items[0].child(NodeKind::ListMark).unwrap().range(), 9..10);
    let strong = tree
        .iter()
        .find(|n| n.kind() == NodeKind::StrongEmphasis)
        .unwrap();
    assert_eq!(&text[strong.range()], "**two**");
    let marks: Vec<_> = strong.children().map(|n| n.range()).collect();
    assert_eq!(marks, [15..17, 20..22]);
    assert_eq!(strong.parent().unwrap().kind(), NodeKind::Paragraph);
    assert_eq!(
        strong.path(),
        "Document/BulletList/ListItem/Paragraph/StrongEmphasis"
    );
    assert_eq!(strong.ancestors().count(), 4);
    assert!(root.parent().is_none());
}

#[test]
fn resolves_positions_by_side() {
    let text = "a *b* c\n";
    let tree = MarkdownTree::parse(text);
    // Inside the emphasis.
    assert_eq!(tree.resolve(3, 0).kind(), NodeKind::Emphasis);
    // At the opening mark: side 1 enters the mark starting there.
    assert_eq!(tree.resolve(2, 1).kind(), NodeKind::EmphasisMark);
    // Side -1 stays out of nodes starting at `pos`.
    assert_eq!(tree.resolve(2, -1).kind(), NodeKind::Paragraph);
    // Past the end: the document.
    assert_eq!(tree.resolve(text.len(), 0).kind(), NodeKind::Document);
    let touched: Vec<_> = tree.iter_range(4..5).map(|n| n.name()).collect();
    assert_eq!(
        touched,
        ["Document", "Paragraph", "Emphasis", "EmphasisMark"]
    );
}

#[test]
fn the_tree_is_always_complete() {
    // Every byte of every block lies inside the document; children nest in
    // their parent and never overlap their siblings.
    for text in [
        "",
        "\n",
        "> > - [ ] *a\n```\nx",
        "| a |\n| - |\n| b",
        "---\nt: 1\n---\n# h\n",
    ] {
        let tree = MarkdownTree::parse(text);
        assert_eq!(tree.root().range(), 0..text.len());
        for node in tree.iter() {
            let mut previous_end = node.from();
            for child in node.children() {
                assert!(
                    child.from() >= previous_end && child.to() <= node.to(),
                    "{text:?}"
                );
                previous_end = child.to();
            }
        }
    }
}

#[test]
fn finds_wiki_links_outside_code() {
    let basic = "see [[Some Note]] here";
    let links = MarkdownTree::parse(basic).wiki_links();
    assert_eq!(links.len(), 1);
    assert_eq!(&basic[links[0].range.clone()], "[[Some Note]]");
    assert_eq!(&basic[links[0].inner.clone()], "Some Note");
    assert!(!links[0].embed);

    let alias = "[[Note|Alias]]";
    let links = MarkdownTree::parse(alias).wiki_links();
    assert_eq!(&alias[links[0].inner.clone()], "Note|Alias");

    let embed = "![[img.png]]";
    let links = MarkdownTree::parse(embed).wiki_links();
    assert_eq!(links.len(), 1);
    assert!(links[0].embed);
    assert_eq!(&embed[links[0].inner.clone()], "img.png");

    let code = "`[[x]]` [[y]]";
    let links = MarkdownTree::parse(code).wiki_links();
    assert_eq!(links.len(), 1);
    assert_eq!(&code[links[0].inner.clone()], "y");
}

#[test]
fn custom_rules_build_their_nodes() {
    // Writer's inline extensions on top of Lezer: escape of any char,
    // inline and display math, emoji shortcodes, dashes, frontmatter.
    let text = "---\na: 1\n---\n\\q $x$ :smile: a -- b --- c\n\n$$\nE\n$$\n";
    let tree = MarkdownTree::parse(text);
    let names: Vec<_> = tree.iter().map(|n| n.name()).collect();
    for name in [
        "Frontmatter",
        "FrontmatterMark",
        "Escape",
        "Math",
        "MathMark",
        "Emoji",
        "Dash",
    ] {
        assert!(names.contains(&name), "missing {name}: {names:?}");
    }
    let dashes: Vec<_> = tree
        .iter()
        .filter(|n| n.kind() == NodeKind::Dash)
        .map(|n| &text[n.range()])
        .collect();
    assert_eq!(dashes, ["--", "---"]);
}

#[test]
fn a_wiki_link_over_several_nodes_wraps_them_in_order() {
    // Writer's pattern crosses line breaks: `[[` and `]]` in two paragraphs.
    let tree = MarkdownTree::parse(
        "[[

]]
",
    );
    assert_eq!(
        tree.dump(),
        "Document 0..7
  WikiLink 0..6
    Paragraph 0..2
    Paragraph 4..6
"
    );
    let tree = MarkdownTree::parse(
        "[[a

b

c]]",
    );
    let link = tree.root().first_child().unwrap();
    assert_eq!(link.kind(), NodeKind::WikiLink);
    let spans: Vec<_> = link.children().map(|n| n.range()).collect();
    assert_eq!(spans, [0..3, 5..6, 8..11]);
}
