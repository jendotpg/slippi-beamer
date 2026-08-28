pub const ELLIPSIS: &str = "...";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub used: usize,
    pub truncated: bool,
}

pub fn fit<'a>(s: &'a str, cols: usize, out: &mut [&'a str]) -> Fit {
    let rows = out.len();
    let mut rest = s.trim();

    if rows == 0 || cols == 0 {
        return Fit {
            used: 0,
            truncated: !rest.is_empty(),
        };
    }

    let mut used = 0;
    while used < rows && !rest.is_empty() {
        if chars(rest) <= cols {
            out[used] = rest;
            return Fit {
                used: used + 1,
                truncated: false,
            };
        }

        if used + 1 == rows {
            let keep = cols.saturating_sub(chars(ELLIPSIS));
            out[used] = rest[..boundary(rest, keep)].trim_end();
            return Fit {
                used: used + 1,
                truncated: true,
            };
        }

        let hard = boundary(rest, cols);
        let cut = if rest[hard..].starts_with(char::is_whitespace) {
            hard
        } else {
            match rest[..hard].rfind(char::is_whitespace) {
                Some(i) => i,
                None => hard, // one long word: split it
            }
        };

        out[used] = rest[..cut].trim_end();
        used += 1;
        rest = rest[cut..].trim_start();
    }

    Fit {
        used,
        truncated: !rest.is_empty(),
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn boundary(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}
