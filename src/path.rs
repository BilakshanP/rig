use std::path::PathBuf;

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }

    PathBuf::from(path)
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "~".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_alone() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
    }

    #[test]
    fn tilde_slash_path() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            expand_tilde("~/foo/bar"),
            PathBuf::from(format!("{home}/foo/bar"))
        );
    }

    #[test]
    fn absolute_path_unchanged() {
        assert_eq!(expand_tilde("/usr/bin"), PathBuf::from("/usr/bin"));
    }

    #[test]
    fn relative_path_unchanged() {
        assert_eq!(expand_tilde("foo/bar"), PathBuf::from("foo/bar"));
    }

    #[test]
    fn tilde_user_not_expanded() {
        assert_eq!(expand_tilde("~foo"), PathBuf::from("~foo"));
    }
}
