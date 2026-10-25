//! 검색 입력 파싱 모듈.
//!
//! 사용자가 입력한 한 줄을 "fd 옵션"과 "검색어"로 나누고,
//! fd 옵션 앞의 `!` 여부로 기본 옵션 사용 여부를 정한다.
//! 외부 크레이트 없이 표준 라이브러리만 사용한다.

/// 입력 한 줄을 나눈 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    /// `--` 앞의 fd 옵션 문자열(앞뒤 공백 제거, `!` 제거 후).
    pub fd_args: String,
    /// `--` 뒤의 검색어(앞뒤 공백 제거). `--` 가 없으면 입력 전체.
    pub pattern: String,
    /// 기본 옵션을 쓸지 여부. fd 옵션이 `!` 로 시작하면 false.
    pub use_defaults: bool,
    /// 단독 `--` 구분자가 있었는지 여부.
    pub has_separator: bool,
}

/// 입력에서 단독 `--` 토큰의 첫 시작 바이트 위치를 찾는다.
///
/// 조건: `--` 바로 앞이 문자열 시작 또는 공백이고, 바로 뒤가 문자열 끝 또는 공백이어야 한다.
/// 공백 판정은 유니코드 공백(`char::is_whitespace`)을 쓴다.
/// 정규식 `(?:^|\s)--(?:\s|$)` 와 같은 결과를 직접 스캔으로 얻는다.
fn find_separator(input: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    for k in 0..chars.len() {
        // 첫 번째 `-` 가 아니면 건너뛴다.
        if chars[k].1 != '-' {
            continue;
        }
        // 바로 다음 글자도 `-` 여야 한다.
        if k + 1 >= chars.len() || chars[k + 1].1 != '-' {
            continue;
        }
        // 앞은 문자열 시작이거나 공백이어야 한다.
        let before_ok = k == 0 || chars[k - 1].1.is_whitespace();
        // 뒤는 문자열 끝이거나 공백이어야 한다.
        let after_ok = k + 2 >= chars.len() || chars[k + 2].1.is_whitespace();
        if before_ok && after_ok {
            return Some(chars[k].0);
        }
    }
    None
}

/// 검색 입력을 fd 옵션과 검색어로 나눈다.
///
/// 규칙
/// - 첫 번째 단독 `--` 토큰을 기준으로 앞은 fd 옵션, 뒤는 검색어다. 각각 trim 한다.
/// - `--` 가 없으면 fd_args 는 빈 문자열, pattern 은 전체 trim 값이다.
///   이 경우 `!` 처리는 하지 않으며 use_defaults 는 true 다.
/// - fd 옵션이 `!` 로 시작하면 `!` 를 제거하고 trim 한 뒤 use_defaults 는 false 다.
pub fn split_query(input: &str) -> ParsedQuery {
    match find_separator(input) {
        None => ParsedQuery {
            fd_args: String::new(),
            pattern: input.trim().to_string(),
            use_defaults: true,
            has_separator: false,
        },
        Some(pos) => {
            // `--` 는 ASCII 2바이트이므로 pos 와 pos + 2 는 항상 문자 경계다.
            let fd_part = input[..pos].trim();
            let pattern = input[pos + 2..].trim().to_string();

            // fd 옵션이 `!` 로 시작하면 기본 옵션을 끈다.
            let (fd_args, use_defaults) = match fd_part.strip_prefix('!') {
                Some(rest) => (rest.trim().to_string(), false),
                None => (fd_part.to_string(), true),
            };

            ParsedQuery {
                fd_args,
                pattern,
                use_defaults,
                has_separator: true,
            }
        }
    }
}

/// 셸 없이 인자 문자열을 나눈다.
///
/// 규칙
/// - 공백(유니코드 공백 포함)으로 나눈다.
/// - 큰따옴표로 묶인 구간은 공백을 포함한 하나의 인자가 된다.
/// - 따옴표 안의 `\` 는 리터럴이다. 따옴표 안에서 `"` 는 항상 구간을 닫는다.
///   따라서 `"C:\dir\"` 는 `C:\dir\` 한 토큰이 된다.
/// - `""` 는 빈 인자 하나다.
/// - 닫히지 않은 따옴표는 Err 를 반환한다.
/// - 입력이 비었거나 공백뿐이면 빈 Vec 를 반환한다.
pub fn tokenize_args(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    // 현재 토큰이 시작되었는지 여부. 빈 따옴표 `""` 를 빈 인자로 살리기 위해 쓴다.
    let mut has_token = false;
    let mut in_quote = false;

    for c in s.chars() {
        if in_quote {
            if c == '"' {
                in_quote = false;
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quote = true;
            has_token = true;
        } else if c.is_whitespace() {
            if has_token {
                out.push(std::mem::take(&mut cur));
                has_token = false;
            }
        } else {
            cur.push(c);
            has_token = true;
        }
    }

    if in_quote {
        return Err("닫히지 않은 따옴표".to_string());
    }
    if has_token {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 기대값 비교를 짧게 쓰기 위한 헬퍼.
    fn sv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // ---- split_query: 문서 표 ----

    #[test]
    fn 표_옵션과_구분자와_검색어() {
        assert_eq!(
            split_query("-e pdf -t f -- 보고서"),
            ParsedQuery {
                fd_args: "-e pdf -t f".to_string(),
                pattern: "보고서".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 표_구분자없음은_전체가_검색어() {
        assert_eq!(
            split_query("보고서"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "보고서".to_string(),
                use_defaults: true,
                has_separator: false,
            }
        );
    }

    #[test]
    fn 표_검색어가_빈_값() {
        assert_eq!(
            split_query("-t d --max-depth 3 --"),
            ParsedQuery {
                fd_args: "-t d --max-depth 3".to_string(),
                pattern: String::new(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 표_느낌표는_기본옵션_끔() {
        assert_eq!(
            split_query("!-H -I -t f -- log"),
            ParsedQuery {
                fd_args: "-H -I -t f".to_string(),
                pattern: "log".to_string(),
                use_defaults: false,
                has_separator: true,
            }
        );
    }

    // ---- split_query: 경계 ----

    #[test]
    fn 경계_구분자만_입력() {
        assert_eq!(
            split_query("--"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: String::new(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_앞뒤_공백() {
        assert_eq!(
            split_query("  -- x  "),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "x".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_선행_구분자() {
        assert_eq!(
            split_query("-- foo"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "foo".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_max_depth는_구분자가_아님() {
        assert_eq!(
            split_query("--max-depth 3 -- x"),
            ParsedQuery {
                fd_args: "--max-depth 3".to_string(),
                pattern: "x".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_하이픈_사이에_문자가_있으면_구분자가_아님() {
        assert_eq!(
            split_query("a--b"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "a--b".to_string(),
                use_defaults: true,
                has_separator: false,
            }
        );
    }

    #[test]
    fn 경계_세_개_하이픈은_구분자가_아님() {
        assert_eq!(
            split_query("---"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "---".to_string(),
                use_defaults: true,
                has_separator: false,
            }
        );
    }

    #[test]
    fn 경계_첫_매치_기준() {
        // 첫 `--` 가 구분자이고 두 번째 `--` 는 검색어에 남는다.
        assert_eq!(
            split_query("-- --"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "--".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_탭_구분() {
        assert_eq!(
            split_query("-t f\t--\tx"),
            ParsedQuery {
                fd_args: "-t f".to_string(),
                pattern: "x".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_느낌표_뒤_공백() {
        assert_eq!(
            split_query("! -t f -- x"),
            ParsedQuery {
                fd_args: "-t f".to_string(),
                pattern: "x".to_string(),
                use_defaults: false,
                has_separator: true,
            }
        );
    }

    #[test]
    fn 경계_구분자없이_느낌표는_검색어() {
        // `--` 가 없으면 `!` 처리를 하지 않는다.
        assert_eq!(
            split_query("!foo"),
            ParsedQuery {
                fd_args: String::new(),
                pattern: "!foo".to_string(),
                use_defaults: true,
                has_separator: false,
            }
        );
    }

    #[test]
    fn 경계_유니코드_공백도_구분자_주변으로_인정() {
        // 전각 공백(U+3000)도 유니코드 공백으로 본다.
        assert_eq!(
            split_query("-t f\u{3000}--\u{3000}보고서"),
            ParsedQuery {
                fd_args: "-t f".to_string(),
                pattern: "보고서".to_string(),
                use_defaults: true,
                has_separator: true,
            }
        );
    }

    // ---- tokenize_args ----

    #[test]
    fn 토큰_평범한_분리() {
        assert_eq!(tokenize_args("a b  c"), Ok(sv(&["a", "b", "c"])));
    }

    #[test]
    fn 토큰_따옴표_공백_인자() {
        assert_eq!(
            tokenize_args("\"hello world\" x"),
            Ok(sv(&["hello world", "x"]))
        );
    }

    #[test]
    fn 토큰_따옴표_끝_백슬래시() {
        // 입력: "C:\dir\" x  -> 토큰: C:\dir\ , x
        assert_eq!(
            tokenize_args(r#""C:\dir\" x"#),
            Ok(sv(&["C:\\dir\\", "x"]))
        );
    }

    #[test]
    fn 토큰_빈_따옴표는_빈_인자() {
        assert_eq!(tokenize_args("\"\""), Ok(sv(&[""])));
    }

    #[test]
    fn 토큰_빈_입력() {
        assert_eq!(tokenize_args(""), Ok(Vec::<String>::new()));
    }

    #[test]
    fn 토큰_공백뿐인_입력() {
        assert_eq!(tokenize_args(" \t  "), Ok(Vec::<String>::new()));
    }

    #[test]
    fn 토큰_닫히지_않은_따옴표() {
        assert_eq!(
            tokenize_args("\"abc def"),
            Err("닫히지 않은 따옴표".to_string())
        );
    }

    #[test]
    fn 토큰_한글_포함() {
        assert_eq!(
            tokenize_args("보고서 \"파일 이름\" 가나다"),
            Ok(sv(&["보고서", "파일 이름", "가나다"]))
        );
    }
}
