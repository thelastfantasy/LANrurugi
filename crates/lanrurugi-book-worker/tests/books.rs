//! End-to-end: the `lanrurugi_book` client driving the real worker binary on generated books of
//! every format. PDF tests need the PDFium library (`mise run fetch-pdfium`) and DjVu tests the
//! DjVuLibre tools; each skips without them rather than failing.

mod common;

use common::*;
use lanrurugi_book::{BookError, BookFormat, ImageKind, PageSource};

fn outline(info: &lanrurugi_book::BookInfo) -> Vec<(&str, u32, u32)> {
    info.outline
        .iter()
        .map(|o| (o.title.as_str(), o.page, o.depth))
        .collect()
}

#[test]
fn pdf_scans_pass_through_and_other_pages_render() {
    if pdfium_lib().is_none() {
        eprintln!("skipped: no PDFium library (mise run fetch-pdfium)");
        return;
    }
    setup();
    let (w, h) = (300, 420);
    let original = jpeg(w, h);
    let pdf = file("book.pdf", &fixture_pdf(&original, w, h));

    let info = lanrurugi_book::inspect(&pdf).unwrap();
    assert_eq!(info.format, BookFormat::Pdf);
    assert_eq!(
        info.pages,
        [
            PageSource::Image(ImageKind::Jpeg),
            PageSource::Render,
            PageSource::Render
        ]
    );
    assert_eq!(info.sizes[0], (w, h));
    assert_eq!(
        outline(&info),
        [("Chapter 1", 0, 0), ("Section 1.1", 1, 1), ("第2話", 2, 0)]
    );
    assert_eq!(lanrurugi_book::read_page(&pdf, 0).unwrap(), original);
    for index in [1, 2] {
        let webp = lanrurugi_book::read_page(&pdf, index).unwrap();
        assert_eq!((&webp[0..4], &webp[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
        assert_eq!(image::load_from_memory(&webp).unwrap().width(), 600);
        assert_eq!(
            lanrurugi_book::read_page(&pdf, index).unwrap(),
            webp,
            "served from cache"
        );
    }
    assert!(matches!(
        lanrurugi_book::read_page(&pdf, 3),
        Err(BookError::PageOutOfRange(3))
    ));
}

fn fixed_layout_opf() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>漫画 第1巻</dc:title><dc:creator>作者</dc:creator><dc:language>ja</dc:language>
    <meta property="rendition:layout">pre-paginated</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="p1" href="text/p1.xhtml" media-type="application/xhtml+xml"/>
    <item id="p2" href="text/p2.xhtml" media-type="application/xhtml+xml"/>
    <item id="p3" href="text/p3.xhtml" media-type="application/xhtml+xml"/>
    <item id="c" href="images/c.jpg" media-type="image/jpeg"/>
    <item id="a" href="images/a.jpg" media-type="image/jpeg"/>
    <item id="b" href="images/b%20x.png" media-type="image/png"/>
  </manifest>
  <spine page-progression-direction="rtl">
    <itemref idref="p1"/><itemref idref="p2"/><itemref idref="p3"/>
  </spine>
</package>"#
}

fn page_xhtml(src: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:xlink="http://www.w3.org/1999/xlink"><head><title>p</title></head><body><svg viewBox="0 0 10 10"><image width="10" height="10" xlink:href="{src}"/></svg></body></html>"#
    )
}

#[test]
fn fixed_layout_epub_follows_the_spine_and_serves_stored_pictures() {
    setup();
    // Spine order c, a, b — not file-name order.
    let (c, a, b) = (jpeg(80, 120), jpeg(81, 120), png(82, 120));
    let nav = r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol>
        <li><a href="text/p1.xhtml">表紙</a></li><li><a href="text/p3.xhtml#x">第一話</a></li></ol></nav></body></html>"#;
    let book = epub(
        fixed_layout_opf(),
        &[
            ("OEBPS/nav.xhtml", nav.as_bytes()),
            (
                "OEBPS/text/p1.xhtml",
                page_xhtml("../images/c.jpg").as_bytes(),
            ),
            (
                "OEBPS/text/p2.xhtml",
                page_xhtml("../images/a.jpg").as_bytes(),
            ),
            (
                "OEBPS/text/p3.xhtml",
                page_xhtml("../images/b%20x.png").as_bytes(),
            ),
            ("OEBPS/images/a.jpg", &a),
            ("OEBPS/images/b x.png", &b),
            ("OEBPS/images/c.jpg", &c),
        ],
    );
    let path = file("comic.epub", &book);
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert_eq!(info.format, BookFormat::Epub);
    assert_eq!(
        info.pages,
        [
            PageSource::Image(ImageKind::Jpeg),
            PageSource::Image(ImageKind::Jpeg),
            PageSource::Image(ImageKind::Png)
        ]
    );
    assert_eq!(info.sizes, [(80, 120), (81, 120), (82, 120)]);
    assert_eq!(info.title.as_deref(), Some("漫画 第1巻"));
    assert_eq!(outline(&info), [("表紙", 0, 0), ("第一話", 2, 0)]);
    assert_eq!(lanrurugi_book::read_page(&path, 0).unwrap(), c);
    assert_eq!(lanrurugi_book::read_page(&path, 1).unwrap(), a);
    assert_eq!(lanrurugi_book::read_page(&path, 2).unwrap(), b);
}

#[test]
fn reflowable_epub_is_typeset_with_its_table_of_contents() {
    setup();
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>小説</dc:title><dc:language>zh-CN</dc:language></metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
    <item id="fig" href="fig.png" media-type="image/png"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
    let para =
        "<p>春天来了，小明在公园里散步，看见樱花开得很漂亮。Mixed text 123 で混在。</p>".repeat(60);
    let c1 = format!(
        "<html><body><h1>第一章　春天</h1>{para}<p><img src=\"fig.png\"/></p></body></html>"
    );
    let c2 = format!("<html><body><h1>第二章　夏天</h1>{para}</body></html>");
    let ncx = r#"<ncx><navMap><navPoint id="1"><navLabel><text>第一章</text></navLabel><content src="c1.xhtml"/></navPoint>
        <navPoint id="2"><navLabel><text>第二章</text></navLabel><content src="c2.xhtml"/></navPoint></navMap></ncx>"#;
    let figure = png(400, 300);
    let path = file(
        "novel.epub",
        &epub(
            opf,
            &[
                ("OEBPS/toc.ncx", ncx.as_bytes()),
                ("OEBPS/c1.xhtml", c1.as_bytes()),
                ("OEBPS/c2.xhtml", c2.as_bytes()),
                ("OEBPS/fig.png", &figure),
            ],
        ),
    );
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert!(info.pages.len() >= 4, "{} pages", info.pages.len());
    assert!(info.pages.iter().all(|p| *p == PageSource::Render));
    assert!(info.sizes.iter().all(|&s| s == (1200, 1700)));
    let toc = outline(&info);
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0], ("第一章", 0, 0));
    assert!(
        toc[1].1 > 1,
        "chapter 2 starts after chapter 1's pages: {toc:?}"
    );
    for page in [0, toc[1].1 as usize] {
        let webp = lanrurugi_book::read_page(&path, page).unwrap();
        assert!(has_ink(&webp), "page {page} is blank");
    }
}

#[test]
fn protected_epubs_are_reported_and_font_obfuscation_is_not_drm() {
    setup();
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata/><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c"/></spine></package>"#;
    let chapter = "<html><body><p>本文です。</p></body></html>".as_bytes();
    let encryption = |algorithm: &str| {
        format!(
            r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="{algorithm}"/></enc:EncryptedData></encryption>"#
        )
    };
    let drm = encryption("http://www.w3.org/2001/04/xmlenc#aes128-cbc");
    let path = file(
        "drm.epub",
        &epub(
            opf,
            &[
                ("META-INF/encryption.xml", drm.as_bytes()),
                ("OEBPS/c.xhtml", chapter),
            ],
        ),
    );
    assert!(matches!(
        lanrurugi_book::inspect(&path),
        Err(BookError::Encrypted)
    ));

    let fonts = encryption("http://www.idpf.org/2008/embedding");
    let path = file(
        "fonts.epub",
        &epub(
            opf,
            &[
                ("META-INF/encryption.xml", fonts.as_bytes()),
                ("OEBPS/c.xhtml", chapter),
            ],
        ),
    );
    assert_eq!(lanrurugi_book::inspect(&path).unwrap().pages.len(), 1);
}

#[test]
fn comic_mobi_pages_follow_the_text_references() {
    setup();
    let (first, second) = (jpeg(70, 100), png(71, 100));
    // Records hold `first` then `second`; the text shows `second` first.
    let html = r#"<html><body><img recindex="00002"/><mbp:pagebreak/><img recindex="00001"/></body></html>"#;
    let path = file(
        "comic.azw3",
        &mobi(
            html,
            &[first.clone(), second.clone()],
            &[(122, b"true"), (100, "作者".as_bytes())],
            0,
        ),
    );
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert_eq!(info.format, BookFormat::Mobi);
    assert_eq!(
        info.pages,
        [
            PageSource::Image(ImageKind::Png),
            PageSource::Image(ImageKind::Jpeg)
        ]
    );
    assert_eq!(info.author.as_deref(), Some("作者"));
    assert_eq!(info.title.as_deref(), Some("Test Book"));
    assert_eq!(lanrurugi_book::read_page(&path, 0).unwrap(), second);
    assert_eq!(lanrurugi_book::read_page(&path, 1).unwrap(), first);
}

#[test]
fn text_mobi_is_typeset_with_heading_chapters() {
    setup();
    let body = "<p>It was a bright cold day in April, and the clocks were striking thirteen. 春天来了。</p>".repeat(40);
    let html = format!("<html><body><h2>Chapter One</h2>{body}<p><img recindex=\"00001\"/></p><h2>Chapter Two</h2>{body}</body></html>");
    let path = file("novel.mobi", &mobi(&html, &[png(200, 150)], &[], 0));
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert!(info.pages.len() >= 2);
    assert!(info.pages.iter().all(|p| *p == PageSource::Render));
    let toc = outline(&info);
    assert_eq!(
        toc.iter().map(|t| t.0).collect::<Vec<_>>(),
        ["Chapter One", "Chapter Two"]
    );
    assert!(has_ink(&lanrurugi_book::read_page(&path, 0).unwrap()));
}

#[test]
fn drm_mobi_is_reported() {
    setup();
    let path = file("drm.azw", &mobi("<p>x</p>", &[], &[], 2));
    assert!(matches!(
        lanrurugi_book::inspect(&path),
        Err(BookError::Encrypted)
    ));
}

#[test]
fn fb2_sections_text_and_pictures() {
    setup();
    use base64::Engine;
    let picture = base64::engine::general_purpose::STANDARD.encode(png(300, 200));
    let para = "<p>吾輩は猫である。名前はまだ無い。<emphasis>どこで</emphasis>生れたかとんと見当がつかぬ。</p>".repeat(30);
    let fb2 = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info><book-title>猫</book-title><author><first-name>漱石</first-name><last-name>夏目</last-name></author><lang>ja</lang></title-info></description>
<body><title><p>猫</p></title>
<section><title><p>一</p></title>{para}<image l:href="#pic"/>
  <section><title><p>一の一</p></title>{para}</section>
</section>
<section><title><p>二</p></title>{para}</section>
</body>
<binary id="pic" content-type="image/png">
{picture}
</binary>
</FictionBook>"##
    );
    let path = file("cat.fb2", fb2.as_bytes());
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert_eq!(info.format, BookFormat::Fb2);
    assert_eq!(info.title.as_deref(), Some("猫"));
    assert_eq!(info.author.as_deref(), Some("漱石 夏目"));
    let toc = outline(&info);
    assert_eq!(
        toc.iter().map(|t| (t.0, t.2)).collect::<Vec<_>>(),
        [("一", 0), ("一の一", 1), ("二", 0)]
    );
    assert!(toc[2].1 > toc[0].1);
    for page in 0..info.pages.len() {
        assert!(
            has_ink(&lanrurugi_book::read_page(&path, page).unwrap()),
            "page {page}"
        );
    }
}

#[test]
fn gbk_text_novel_with_chapters() {
    setup();
    let mut text = String::from("书名：测试\n\n");
    for chapter in ["第一章 春天", "第二章 夏天", "第三章 秋天"] {
        text.push_str(chapter);
        text.push('\n');
        for _ in 0..40 {
            text.push_str("　　春天来了，小明在公园里散步，看见樱花开得很漂亮。他想起了去年和朋友们一起赏花的日子。\n");
        }
    }
    let (gbk, _, _) = encoding_rs::GBK.encode(&text);
    let path = file("novel.txt", &gbk);
    let info = lanrurugi_book::inspect(&path).unwrap();
    assert_eq!(info.format, BookFormat::Txt);
    let toc = outline(&info);
    assert_eq!(
        toc.iter().map(|t| t.0).collect::<Vec<_>>(),
        ["第一章 春天", "第二章 夏天", "第三章 秋天"]
    );
    assert!(toc.windows(2).all(|w| w[0].1 < w[1].1), "{toc:?}");
    assert!(has_ink(
        &lanrurugi_book::read_page(&path, toc[1].1 as usize).unwrap()
    ));
}

#[test]
fn broken_books_fail_cleanly_and_the_pool_survives() {
    setup();
    for (name, bytes) in [
        ("broken.epub", &b"PK\x03\x04garbage"[..]),
        ("broken.mobi", &b"short"[..]),
        ("broken.fb2", &b"\x00\x01 not xml at all \xff"[..]),
    ] {
        let path = file(name, bytes);
        assert!(lanrurugi_book::inspect(&path).is_err(), "{name}");
    }
    let path = file("ok.txt", "第一章\n内容。\n".repeat(400).as_bytes());
    assert!(!lanrurugi_book::inspect(&path).unwrap().pages.is_empty());
}

#[test]
fn djvu_pages_render_and_bookmarks_import() {
    let tools = ["c44", "djvm", "djvused", "ddjvu"];
    if tools.iter().any(|t| {
        std::process::Command::new(t)
            .arg("--help")
            .output()
            .is_err()
    }) {
        eprintln!("skipped: DjVuLibre tools not installed");
        return;
    }
    let dir = setup();
    let pages: Vec<_> = (0..2)
        .map(|i| {
            let ppm = dir.join(format!("p{i}.ppm"));
            let page = image::RgbImage::from_fn(200, 300, |x, y| {
                image::Rgb([(x + i * 60) as u8, y as u8, 90])
            });
            // Binary PPM (P6) by hand: `image` writes PAM (P7) for RGB, which c44 cannot read.
            let mut bytes = format!("P6\n{} {}\n255\n", page.width(), page.height()).into_bytes();
            bytes.extend_from_slice(page.as_raw());
            std::fs::write(&ppm, bytes).unwrap();
            let djvu = dir.join(format!("p{i}.djvu"));
            assert!(std::process::Command::new("c44")
                .arg(&ppm)
                .arg(&djvu)
                .status()
                .unwrap()
                .success());
            djvu
        })
        .collect();
    let book = dir.join("scan.djvu");
    assert!(std::process::Command::new("djvm")
        .arg("-c")
        .arg(&book)
        .args(&pages)
        .status()
        .unwrap()
        .success());
    let outline_file = dir.join("outline.txt");
    std::fs::write(
        &outline_file,
        "(bookmarks (\"第一章\" \"#1\") (\"第二章\" \"#2\"))",
    )
    .unwrap();
    assert!(std::process::Command::new("djvused")
        .arg(&book)
        .args([
            "-e",
            &format!("set-outline {}", outline_file.display()),
            "-s"
        ])
        .status()
        .unwrap()
        .success());

    let info = lanrurugi_book::inspect(&book).unwrap();
    assert_eq!(info.format, BookFormat::Djvu);
    assert_eq!(info.pages, [PageSource::Render, PageSource::Render]);
    assert_eq!(info.sizes[0], (1800, 2700));
    assert_eq!(outline(&info), [("第一章", 0, 0), ("第二章", 1, 0)]);
    let page = image::load_from_memory(&lanrurugi_book::read_page(&book, 1).unwrap()).unwrap();
    assert_eq!(page.width(), 600);
}
