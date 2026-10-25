//! fdpick 내장 매처.
//!
//! fd 가 스트리밍으로 내놓는 경로 문자열을 nucleo 의 Injector 로 받아 백그라운드 스레드에서
//! 증분 fuzzy 매칭하고, 정렬 규칙(점수 -> 파일명 매치 여부 -> 짧은 경로)을 적용한 상위 N개
//! 스냅샷을 돌려준다.
//!
//! 필요한 의존성: `nucleo = "0.5"`, `unicode-segmentation = "1"`.

use std::sync::{Arc, Mutex};

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo, Utf32String};
use unicode_segmentation::UnicodeSegmentation;

/// 화면에 보여줄 매치 한 건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchItem {
    pub path: String,
    /// path 안의 매치된 글자 위치(유니코드 코드포인트 인덱스), 오름차순, 중복 없음.
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<MatchItem>,
    /// 패턴에 매치된 전체 개수(nucleo 스냅샷 기준; 매칭이 진행 중이면 최종값보다 작을 수 있다).
    pub matched: u32,
    /// 지금까지 inject 된 총 항목 수.
    pub scanned: u32,
}

/// 경로 모드 매처 설정. 구분자(`\`, `/`)를 인식해 파일명 시작에 보너스를 준다.
fn path_config() -> Config {
    Config::DEFAULT.match_paths()
}

pub struct Matcher {
    nucleo: Nucleo<String>,
    /// 직전에 설정한 (trim 된) 패턴. append 최적화 판정용.
    last_pattern: String,
    /// 상위 항목 재점수/indices 계산용. snapshot(&self) 에서 쓰므로 Mutex 로 감싼다.
    scorer: Mutex<nucleo::Matcher>,
}

impl Matcher {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        // 스레드 수 제한 이유: nucleo 기본값은 논리 코어 전부를 쓰는데, 100만 개 규모 재매칭 중에
        // 전 코어가 포화되면 UI/웹뷰/fd 프로세스가 굼떠진다. 코어 2개는 남기고 최대 6개로 제한.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        let threads = cores.saturating_sub(2).clamp(1, 6);
        Self {
            nucleo: Nucleo::new(path_config(), notify, Some(threads), 1),
            last_pattern: String::new(),
            scorer: Mutex::new(nucleo::Matcher::new(path_config())),
        }
    }

    /// 항목을 모두 비운다. 기존 Injector 는 계속 push 해도 안전하지만 더 이상 이 매처에
    /// 반영되지 않는다(이전 항목 메모리는 옛 Injector 가 모두 drop 될 때 해제된다).
    pub fn clear(&mut self) {
        self.nucleo.restart(true);
    }

    pub fn injector(&self) -> Injector {
        Injector {
            inner: self.nucleo.injector(),
        }
    }

    /// 스마트 케이스: 검색어에 대문자가 있으면 대소문자 구분.
    /// 이전 패턴의 연장(접두 일치)이면 append 최적화로 기존 매치 집합만 재검사한다.
    pub fn set_pattern(&mut self, pattern: &str) {
        let pattern = pattern.trim();
        let append = pattern.starts_with(self.last_pattern.as_str());
        self.nucleo.pattern.reparse(
            0,
            pattern,
            CaseMatching::Smart,
            Normalization::Smart,
            append,
        );
        self.last_pattern.clear();
        self.last_pattern.push_str(pattern);
    }

    /// nucleo tick. 스냅샷이 바뀌었으면 true. 매칭 진행 여부까지 알고 싶으면 `is_running` 사용.
    pub fn tick(&mut self, timeout_ms: u64) -> bool {
        self.tick_status(timeout_ms).0
    }

    /// (changed, running). running 이 false 면 현재 패턴/항목에 대한 매칭이 끝난 것.
    pub fn tick_status(&mut self, timeout_ms: u64) -> (bool, bool) {
        let st = self.nucleo.tick(timeout_ms);
        (st.changed, st.running)
    }

    /// 매칭이 백그라운드에서 아직 진행 중인지(tick 호출 필요 여부).
    pub fn is_running(&mut self) -> bool {
        self.tick_status(0).1
    }

    /// 현재 스냅샷에서 상위 `limit` 개를 정렬해 돌려준다.
    ///
    /// 정렬: nucleo 점수 -> 파일명에 매치 글자가 있는 쪽 -> 짧은 경로.
    /// nucleo 의 `Snapshot` 은 점수를 공개하지 않으므로(Match.score 는 비공개 접근 불가),
    /// 후보에 대해 `Pattern::indices` 로 점수와 indices 를 직접 다시 계산한다(nucleo 와 같은
    /// Config/함수라 점수가 동일). 비용: 후보 최대 `limit * 4` 건의 재매칭(수백 건, 1ms 미만).
    ///
    /// 한계: nucleo 는 점수 순으로만 정렬해 두므로, limit 경계의 동점 구간이 `limit * 4` 보다
    /// 길면 그 밖의 동점 항목은 (파일명 매치/길이 기준으로 더 위여도) 후보에서 빠진다.
    pub fn snapshot(&self, limit: usize, empty_limit: usize) -> Snapshot {
        let injector = self.nucleo.injector();
        let scanned = injector.injected_items();

        if self.last_pattern.is_empty() {
            // 빈 검색어: 입력 순서대로 앞쪽 empty_limit 개. indices 는 비움.
            let n = (empty_limit as u64).min(scanned as u64) as u32;
            let mut items = Vec::with_capacity(n as usize);
            for i in 0..scanned {
                if items.len() as u32 >= n {
                    break;
                }
                // boxcar 는 비연속 초기화가 가능하므로 미초기화(None) 는 건너뛴다.
                if let Some(it) = injector.get(i) {
                    items.push(MatchItem {
                        path: it.data.clone(),
                        indices: Vec::new(),
                    });
                }
            }
            return Snapshot {
                items,
                matched: scanned,
                scanned,
            };
        }

        let snap = self.nucleo.snapshot();
        let matched = snap.matched_item_count();
        if limit == 0 || matched == 0 {
            return Snapshot {
                items: Vec::new(),
                matched,
                scanned,
            };
        }

        let pat = snap.pattern().column_pattern(0);
        let mut scorer = self.scorer.lock().unwrap_or_else(|e| e.into_inner());

        struct Cand {
            score: u32,
            file_hit: bool,
            path_chars: usize,
            path: String,
            indices: Vec<u32>,
        }
        let mut cands: Vec<Cand> = Vec::new();
        let hard_cap = limit.saturating_mul(4).min(matched as usize);
        let mut boundary: Option<u32> = None;

        for n in 0..hard_cap {
            let Some(item) = snap.get_matched_item(n as u32) else {
                continue;
            };
            let mut idx = Vec::new();
            let hay = item.matcher_columns[0].slice(..);
            let Some(score) = pat.indices(hay, &mut scorer, &mut idx) else {
                // 스냅샷과 패턴이 어긋난 드문 순간(경합)에는 건너뛴다.
                continue;
            };
            if n + 1 == limit {
                boundary = Some(score);
            }
            // limit 이후에는 경계 점수와 같은 동점 항목만 계속 읽는다.
            if n >= limit && boundary.is_none_or(|b| score != b) {
                break;
            }
            idx.sort_unstable();
            idx.dedup();
            let path = item.data;
            // idx 는 Utf32Str(그래핌) 위치이므로 파일명 판정도 같은 단위로 한다.
            let file_start = filename_start_units(path, item.matcher_columns[0].len());
            cands.push(Cand {
                score,
                file_hit: idx.iter().any(|&i| (i as usize) >= file_start),
                path_chars: path.chars().count(),
                path: path.clone(),
                indices: idx,
            });
        }

        // 안정 정렬: 모든 키가 같으면 nucleo 의 원래 순서를 유지한다.
        cands.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.file_hit.cmp(&a.file_hit))
                .then(a.path_chars.cmp(&b.path_chars))
        });
        cands.truncate(limit);

        let items = cands
            .into_iter()
            .map(|c| {
                let indices = to_char_indices(&c.path, &c.indices);
                MatchItem {
                    path: c.path,
                    indices,
                }
            })
            .collect();
        Snapshot {
            items,
            matched,
            scanned,
        }
    }
}

#[derive(Clone)]
pub struct Injector {
    inner: nucleo::Injector<String>,
}

impl Injector {
    pub fn push(&self, path: String) {
        self.inner.push(path, |s, cols| {
            cols[0] = Utf32String::from(s.as_str());
        });
    }

    /// nucleo 의 notify 는 push 마다 호출되므로, notify 쪽에서 디바운스해야 한다.
    pub fn push_many(&self, paths: Vec<String>) {
        for p in paths {
            self.push(p);
        }
    }
}

/// 파일명(마지막 경로 세그먼트, 끝 구분자 무시) 시작 위치를 Utf32Str 단위(그래핌)로 돌려준다.
fn filename_start_units(path: &str, units_len: usize) -> usize {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let byte_start = trimmed.rfind(['\\', '/']).map_or(0, |p| p + 1);
    if path.is_ascii() {
        return byte_start;
    }
    // 비 ASCII: byte_start 이전 그래핌 수. 구분자는 ASCII 라 그래핌 경계와 일치한다.
    let n = path[..byte_start].graphemes(true).count();
    n.min(units_len)
}

/// nucleo 의 indices 는 Utf32Str 위치다. unicode-segmentation 이 켜져 있으면 그래핌 단위
/// (그래핌의 첫 코드포인트만 보관)라서 NFD 한글/결합 문자/ZWJ 이모지에서 char 위치와 어긋난다.
/// 매치된 그래핌에 속한 모든 코드포인트 위치로 확장해 char 인덱스(오름차순, 중복 없음)로 바꾼다.
fn to_char_indices(path: &str, idx: &[u32]) -> Vec<u32> {
    if path.is_ascii() {
        return idx.to_vec();
    }
    let mut out = Vec::with_capacity(idx.len());
    let mut want = idx.iter().copied().peekable();
    let mut char_pos = 0u32;
    for (g, grapheme) in path.graphemes(true).enumerate() {
        let n = grapheme.chars().count() as u32;
        let hit = matches!(want.peek(), Some(&w) if w as usize == g);
        if hit {
            want.next();
            out.extend(char_pos..char_pos + n);
        }
        char_pos += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn new_matcher() -> Matcher {
        Matcher::new(Arc::new(|| {}))
    }

    /// tick 을 돌려 매칭이 끝날 때까지(상한 5초) 기다린다.
    fn settle(m: &mut Matcher) {
        let t = Instant::now();
        loop {
            let (_, running) = m.tick_status(10);
            if !running {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "매칭이 5초 안에 끝나지 않음");
        }
    }

    fn run(m: &mut Matcher, pat: &str, limit: usize) -> Snapshot {
        m.set_pattern(pat);
        settle(m);
        m.snapshot(limit, 100)
    }

    fn fill(m: &Matcher, paths: &[&str]) {
        let inj = m.injector();
        for p in paths {
            inj.push((*p).to_string());
        }
    }

    fn paths(s: &Snapshot) -> Vec<&str> {
        s.items.iter().map(|i| i.path.as_str()).collect()
    }

    #[test]
    fn 파일명_매치가_디렉터리_매치보다_위() {
        let mut m = new_matcher();
        fill(&m, &[r"C:\proj\report\x.txt", r"C:\other\report.txt"]);
        let s = run(&mut m, "report", 10);
        assert_eq!(paths(&s), vec![r"C:\other\report.txt", r"C:\proj\report\x.txt"]);
        assert_eq!(s.matched, 2);
        assert_eq!(s.scanned, 2);
    }

    #[test]
    fn 동점이면_파일명_매치_다음_짧은_경로() {
        // 점수 동점을 만들기 위해 동일 구조로 파일명/디렉터리 위치만 다르게 한 쌍과
        // 길이만 다른 쌍을 따로 검증한다. 정렬 규칙 자체는 비교 키로 직접 확인한다.
        let mut m = new_matcher();
        fill(
            &m,
            &[
                r"C:\aaaaaaaa\foo.txt",
                r"C:\a\foo.txt",
                r"C:\aaaa\foo.txt",
            ],
        );
        let s = run(&mut m, "foo.txt", 10);
        // 같은 파일명 매치이므로 점수가 같고, 이 경우 짧은 경로가 먼저.
        assert_eq!(
            paths(&s),
            vec![r"C:\a\foo.txt", r"C:\aaaa\foo.txt", r"C:\aaaaaaaa\foo.txt"]
        );

        // 파일명 매치 여부 우선: 두 항목의 점수가 같을 때만 의미가 있으므로 같은 후보 안에서
        // 파일명 히트가 있는 쪽이 짧은 경로보다 앞서는지 내부 정렬 키로 확인한다.
        let mut m2 = new_matcher();
        fill(&m2, &[r"C:\x\bar\q.txt", r"C:\x\qqqqqqqq\bar.txt"]);
        let s2 = run(&mut m2, "bar", 10);
        assert_eq!(s2.items[0].path, r"C:\x\qqqqqqqq\bar.txt");
        assert!(s2.items[0].indices.iter().all(|&i| i >= 14));

        // 점수와 길이가 같은 쌍: 파일명에 매치된 쪽이 위(입력 순서와 반대로 넣어 확인).
        let mut m3 = new_matcher();
        fill(&m3, &[r"C:\bar\q", r"C:\q\bar"]);
        let s3 = run(&mut m3, "bar", 10);
        assert_eq!(paths(&s3), vec![r"C:\q\bar", r"C:\bar\q"]);
    }

    #[test]
    fn 동점_구간_재정렬_limit_경계() {
        // 모든 항목이 같은 점수(동일 구조)이고 길이만 다르다. limit=3 이어도 경계 밖
        // 짧은 경로가 후보에 들어와 올바르게 위로 올라와야 한다.
        let mut m = new_matcher();
        let inj = m.injector();
        for n in (1..=30).rev() {
            inj.push(format!(r"C:\{}\foo", "d".repeat(n)));
        }
        let s = run(&mut m, "foo", 3);
        let lens: Vec<usize> = s.items.iter().map(|i| i.path.len()).collect();
        let mut sorted = lens.clone();
        sorted.sort();
        assert_eq!(lens, sorted);
        assert_eq!(s.items[0].path, r"C:\d\foo");
    }

    #[test]
    fn 스마트_케이스() {
        let mut m = new_matcher();
        fill(&m, &[r"C:\Docs\Readme.md", r"C:\docs\readme.md"]);
        let s = run(&mut m, "readme", 10);
        assert_eq!(s.matched, 2);
        let s = run(&mut m, "Readme", 10);
        assert_eq!(paths(&s), vec![r"C:\Docs\Readme.md"]);
        let s = run(&mut m, "README", 10);
        assert_eq!(s.matched, 0);
    }

    #[test]
    fn 한글_매칭과_char_단위_indices() {
        let mut m = new_matcher();
        // 앞에 한글/이모지(비 BMP)를 두어 바이트/UTF-16/char 인덱스가 서로 달라지게 한다.
        fill(&m, &[r"C:\내문서\😀보고서_최종.docx", r"C:\other\a.txt"]);
        let s = run(&mut m, "보고서", 10);
        assert_eq!(s.items.len(), 1);
        let it = &s.items[0];
        let chars: Vec<char> = it.path.chars().collect();
        let got: String = it.indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(got, "보고서");
        let start = chars.iter().position(|&c| c == '보').unwrap() as u32;
        assert_eq!(it.indices, vec![start, start + 1, start + 2]);
    }

    #[test]
    fn nfd_한글도_char_인덱스로_변환() {
        // 자모 분리(NFD) 문자열: 그래핌 단위 인덱스가 char 인덱스로 확장되는지 확인.
        let nfd = "C:\\\u{1112}\u{1161}\u{11AB}\\\u{1100}\u{1161}.txt"; // 한 / 가
        let mut m = new_matcher();
        fill(&m, &[nfd]);
        let s = run(&mut m, "txt", 10);
        let it = &s.items[0];
        let chars: Vec<char> = it.path.chars().collect();
        let got: String = it.indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(got, "txt");
        assert!(it.indices.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn 증분_입력_결과가_처음부터_매칭한_결과와_같다() {
        let data: Vec<String> = (0..2000)
            .map(|i| match i % 4 {
                0 => format!(r"C:\proj\abc_{i}\file{i}.txt"),
                1 => format!(r"C:\proj\ab{i}\xc{i}.rs"),
                2 => format!(r"C:\a\b\c\{i}.md"),
                _ => format!(r"C:\zzz\{i}\q"),
            })
            .collect();
        let refs: Vec<&str> = data.iter().map(|s| s.as_str()).collect();

        let mut inc = new_matcher();
        fill(&inc, &refs);
        let _ = run(&mut inc, "a", 50);
        let _ = run(&mut inc, "ab", 50);
        let a = run(&mut inc, "abc", 50);

        let mut fresh = new_matcher();
        fill(&fresh, &refs);
        let b = run(&mut fresh, "abc", 50);

        assert_eq!(a, b);
        assert!(a.matched > 0);

        // 지우는 방향(비 append)도 처음부터 한 결과와 같아야 한다.
        let back = run(&mut inc, "ab", 50);
        let mut fresh2 = new_matcher();
        fill(&fresh2, &refs);
        let direct = run(&mut fresh2, "ab", 50);
        assert_eq!(back, direct);
    }

    #[test]
    fn 빈_검색어는_입력_순서대로_empty_limit_개() {
        let mut m = new_matcher();
        fill(&m, &["z", "y", "x", "w"]);
        m.set_pattern("   ");
        settle(&mut m);
        let s = m.snapshot(10, 2);
        assert_eq!(paths(&s), vec!["z", "y"]);
        assert!(s.items.iter().all(|i| i.indices.is_empty()));
        assert_eq!(s.matched, 4);
        assert_eq!(s.scanned, 4);

        // 검색 후 다시 비우면 빈 검색어 동작으로 복귀한다.
        let _ = run(&mut m, "x", 10);
        let s = run(&mut m, "", 10);
        assert_eq!(s.items.len(), 4);
    }

    #[test]
    fn clear_후_재주입() {
        let mut m = new_matcher();
        fill(&m, &["C:\\a\\one.txt", "C:\\a\\two.txt"]);
        let old = m.injector();
        let s = run(&mut m, "txt", 10);
        assert_eq!(s.matched, 2);

        m.clear();
        // 옛 Injector 는 push 해도 안전하고 새 목록에는 영향이 없어야 한다.
        old.push("C:\\a\\ghost.txt".into());
        fill(&m, &["C:\\b\\three.txt"]);
        let s = run(&mut m, "txt", 10);
        assert_eq!(paths(&s), vec!["C:\\b\\three.txt"]);
        assert_eq!(s.scanned, 1);
    }

    #[test]
    fn 주입_중_다른_스레드에서_tick_snapshot_안전() {
        let notified = Arc::new(AtomicUsize::new(0));
        let n2 = notified.clone();
        let mut m = Matcher::new(Arc::new(move || {
            n2.fetch_add(1, Ordering::Relaxed);
        }));
        m.set_pattern("fi");
        let inj = m.injector();
        let h = std::thread::spawn(move || {
            for chunk in 0..50 {
                let v: Vec<String> = (0..1000)
                    .map(|i| format!(r"C:\dir{}\file{}.txt", chunk, i))
                    .collect();
                inj.push_many(v);
            }
        });
        let t = Instant::now();
        while !h.is_finished() {
            m.tick(1);
            let s = m.snapshot(20, 20);
            assert!(s.items.len() <= 20);
            assert!(t.elapsed() < Duration::from_secs(30));
        }
        h.join().unwrap();
        settle(&mut m);
        let s = m.snapshot(20, 20);
        assert_eq!(s.scanned, 50_000);
        assert_eq!(s.matched, 50_000);
        assert_eq!(s.items.len(), 20);
        assert!(notified.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn 끝_구분자는_파일명_판정에서_무시() {
        assert_eq!(filename_start_units(r"C:\a\bb\", 100), 5);
        assert_eq!(filename_start_units("a/b", 100), 2);
        assert_eq!(filename_start_units("abc", 100), 0);
    }
}
