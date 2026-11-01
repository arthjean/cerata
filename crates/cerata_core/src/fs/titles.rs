//! Sidebar titles, read off the listing path in batches.
//!
//! Ported from Writer `fs.rs:50-112` (`extract_title`) and implements Writer's
//! unshipped `SPECs/slow-storage-resilience-spec.md` (defect 12: Writer read
//! titles synchronously inside `read_directory`, so one iCloud placeholder or
//! stalled network mount froze the sidebar). Listings return no titles; the
//! caller then starts [`spawn_title_batches`], which reads titles on
//! background threads, gives each batch at most [`TITLE_BATCH_DEADLINE`],
//! and stops as soon as the workspace epoch moves.

use crate::epoch::EpochToken;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Bytes read from the start of a file to find its title.
pub const TITLE_PROBE_BYTES: usize = 4096;
/// Wall-clock budget of one batch; slower files are reported as timed out.
pub const TITLE_BATCH_DEADLINE: Duration = Duration::from_secs(2);
/// Files per batch.
pub const TITLE_BATCH_SIZE: usize = 64;
/// Reader threads per batch, so one slow file does not serialize the rest.
const READERS_PER_BATCH: usize = 4;

/// Title of a Markdown file: the frontmatter `title:` field, else a leading
/// `# ` heading (the first non-blank line). Reads at most 4 KB.
pub fn extract_title(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; TITLE_PROBE_BYTES];
    let read = file.read(&mut buf).ok()?;
    let text = match std::str::from_utf8(&buf[..read]) {
        Ok(text) => text,
        // The 4 KB cut may split a character: keep the valid prefix.
        Err(error) => std::str::from_utf8(&buf[..error.valid_up_to()]).ok()?,
    };
    title_from_text(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// [`extract_title`] on text already in memory. The frontmatter is found by
/// the editor's parser (`cerata_markdown::frontmatter`), so the sidebar and
/// the editor never disagree on where it ends (US-010).
pub fn title_from_text(text: &str) -> Option<String> {
    let (frontmatter, body) = cerata_markdown::frontmatter::split(text);
    if let Some(frontmatter) = frontmatter {
        for line in frontmatter.lines() {
            if let Some(value) = line.trim().strip_prefix("title:") {
                let value = value.trim();
                let title = value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .unwrap_or(value);
                if !title.is_empty() {
                    return Some(title.to_owned());
                }
            }
        }
    }
    leading_h1(body)
}

fn leading_h1(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let title = line.strip_prefix("# ")?.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// Titles of one batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleBatch {
    pub epoch: u64,
    /// Every path of the batch, with `None` for no title or not read in time.
    pub titles: Vec<(PathBuf, Option<String>)>,
    /// Paths whose read did not finish before the batch deadline.
    pub timed_out: Vec<PathBuf>,
}

/// Reads the titles of `paths` in batches on a background thread and hands
/// each batch to `on_batch`. Stops, without reporting, once `token` is stale.
pub fn spawn_title_batches(
    paths: Vec<PathBuf>,
    token: EpochToken,
    on_batch: impl FnMut(TitleBatch) + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    spawn_title_batches_with(
        paths,
        token,
        TITLE_BATCH_SIZE,
        TITLE_BATCH_DEADLINE,
        Arc::new(extract_title),
        on_batch,
    )
}

pub(crate) type TitleReader = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

pub(crate) fn spawn_title_batches_with(
    paths: Vec<PathBuf>,
    token: EpochToken,
    batch_size: usize,
    deadline: Duration,
    read: TitleReader,
    mut on_batch: impl FnMut(TitleBatch) + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("cerata-titles".into())
        .spawn(move || {
            for chunk in paths.chunks(batch_size.max(1)) {
                if token.is_stale() {
                    return;
                }
                let batch = read_batch(chunk, &read, deadline, token.epoch());
                if token.is_stale() {
                    return;
                }
                on_batch(batch);
            }
        })
}

fn read_batch(paths: &[PathBuf], read: &TitleReader, deadline: Duration, epoch: u64) -> TitleBatch {
    let started = Instant::now();
    let shared: Arc<Vec<PathBuf>> = Arc::new(paths.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    for _ in 0..READERS_PER_BATCH.min(paths.len()) {
        let (shared, next, sender, read) =
            (shared.clone(), next.clone(), sender.clone(), read.clone());
        // Readers stuck in a blocking syscall are left behind on purpose: a
        // read blocked in the kernel cannot be canceled from user space.
        let spawned = std::thread::Builder::new()
            .name("cerata-title-reader".into())
            .spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(path) = shared.get(index) else { break };
                    if sender.send((index, read(path))).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            log::warn!("could not start a title reader: {error}");
        }
    }
    drop(sender);

    let mut titles: Vec<Option<Option<String>>> = vec![None; paths.len()];
    let mut received = 0;
    while received < paths.len() {
        let Some(left) = deadline.checked_sub(started.elapsed()) else {
            break;
        };
        match receiver.recv_timeout(left) {
            Ok((index, title)) => {
                if let Some(slot) = titles.get_mut(index) {
                    *slot = Some(title);
                    received += 1;
                }
            }
            Err(_) => break,
        }
    }

    let mut timed_out = Vec::new();
    let titles = paths
        .iter()
        .zip(titles)
        .map(|(path, title)| match title {
            Some(title) => (path.clone(), title),
            None => {
                log::warn!("title read exceeded {deadline:?}: {}", path.display());
                timed_out.push(path.clone());
                (path.clone(), None)
            }
        })
        .collect();
    TitleBatch {
        epoch,
        titles,
        timed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epoch::EpochCounter;
    use std::fs;
    use tempfile::TempDir;

    fn title_of(content: &str) -> Option<String> {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.md");
        fs::write(&path, content).unwrap();
        extract_title(&path)
    }

    // Ported from Writer `fs.rs` title tests.

    #[test]
    fn extract_title_from_h1() {
        assert_eq!(title_of("# My Title\n\nBody").as_deref(), Some("My Title"));
    }

    #[test]
    fn extract_title_from_frontmatter() {
        assert_eq!(
            title_of("---\ntitle: From FM\n---\n# Heading").as_deref(),
            Some("From FM")
        );
    }

    #[test]
    fn extract_title_frontmatter_quoted() {
        assert_eq!(
            title_of("---\ntitle: \"Quoted\"\n---\n").as_deref(),
            Some("Quoted")
        );
        assert_eq!(
            title_of("---\ntitle: 'Single'\n---\n").as_deref(),
            Some("Single")
        );
    }

    #[test]
    fn extract_title_frontmatter_beats_h1() {
        assert_eq!(title_of("---\ntitle: FM\n---\n# H1").as_deref(), Some("FM"));
    }

    #[test]
    fn extract_title_h1_after_empty_frontmatter() {
        assert_eq!(
            title_of("---\ntags: [a]\n---\n\n# After").as_deref(),
            Some("After")
        );
    }

    #[test]
    fn extract_title_none_for_empty_file() {
        assert_eq!(title_of(""), None);
    }

    #[test]
    fn extract_title_none_when_no_heading() {
        assert_eq!(title_of("Just text\n# Later heading"), None);
    }

    #[test]
    fn extract_title_with_crlf_and_bom() {
        assert_eq!(
            title_of("\u{feff}---\r\ntitle: Win\r\n---\r\n# H").as_deref(),
            Some("Win")
        );
    }

    #[test]
    fn extract_title_frontmatter_closed_at_end_of_file() {
        // The editor's rule: a closing `---` with no newline after it.
        assert_eq!(title_of("---\ntitle: End\n---").as_deref(), Some("End"));
        assert_eq!(title_of("---\ntitle: Open\n# H"), None);
    }

    // Batches.

    fn collect(
        paths: Vec<PathBuf>,
        token: EpochToken,
        read: TitleReader,
        deadline: Duration,
    ) -> Vec<TitleBatch> {
        let (sender, receiver) = mpsc::channel();
        spawn_title_batches_with(paths, token, 2, deadline, read, move |batch| {
            sender.send(batch).unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
        receiver.try_iter().collect()
    }

    #[test]
    fn batches_report_every_title_in_order() {
        let dir = TempDir::new().unwrap();
        let mut paths = Vec::new();
        for i in 0..5 {
            let path = dir.path().join(format!("{i}.md"));
            fs::write(&path, format!("# Title {i}")).unwrap();
            paths.push(path);
        }
        let (sender, receiver) = mpsc::channel();
        spawn_title_batches(paths.clone(), EpochCounter::new().token(), move |batch| {
            sender.send(batch).unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
        let batches: Vec<TitleBatch> = receiver.try_iter().collect();
        let titles: Vec<_> = batches.into_iter().flat_map(|b| b.titles).collect();
        assert_eq!(titles.len(), 5);
        for (i, (path, title)) in titles.iter().enumerate() {
            assert_eq!(path, &paths[i]);
            assert_eq!(title.as_deref(), Some(format!("Title {i}").as_str()));
        }
    }

    #[test]
    fn a_slow_file_times_out_without_holding_the_batch() {
        let slow = PathBuf::from("slow.md");
        let fast = PathBuf::from("fast.md");
        let read: TitleReader = Arc::new(|path: &Path| {
            if path == Path::new("slow.md") {
                std::thread::sleep(Duration::from_secs(5));
            }
            Some(path.display().to_string())
        });
        let started = Instant::now();
        let batches = collect(
            vec![slow.clone(), fast.clone()],
            EpochCounter::new().token(),
            read,
            Duration::from_millis(200),
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].timed_out, vec![slow.clone()]);
        assert_eq!(
            batches[0].titles,
            vec![(slow, None), (fast, Some("fast.md".to_owned()))]
        );
    }

    #[test]
    fn a_stale_epoch_stops_further_batches() {
        let counter = EpochCounter::new();
        let token = counter.token();
        let advance = counter.clone();
        let read: TitleReader = Arc::new(move |_path: &Path| {
            advance.advance();
            None
        });
        let paths = (0..6).map(|i| PathBuf::from(format!("{i}.md"))).collect();
        let batches = collect(paths, token, read, Duration::from_secs(1));
        assert!(batches.is_empty());
    }
}
