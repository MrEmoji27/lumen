//! KeePass: open a .kdbx from this PC, keep it in memory only, fill logins from it.

use std::path::Path;

use keepass::{Database, DatabaseKey, error::DatabaseOpenError};

pub struct Entry {
    pub title: String,
    pub username: String,
    pub url: String,
    pub password: String,
}

pub struct Vault {
    entries: Vec<Entry>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault").field("entries", &self.entries.len()).finish()
    }
}

impl Vault {
    pub fn open(path: &Path, password: &str) -> Result<Vault, String> {
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let db = Database::open(&mut file, DatabaseKey::new().with_password(password)).map_err(|e| {
            if matches!(e, DatabaseOpenError::Key(keepass::error::DatabaseKeyError::IncorrectKey)) {
                crate::privacy_stats::wrong_password();
            }
            e.to_string()
        })?;
        let entries = db
            .iter_all_entries()
            .map(|e| Entry {
                title: e.get_title().unwrap_or("").to_string(),
                username: e.get_username().unwrap_or("").to_string(),
                url: e.get_url().unwrap_or("").to_string(),
                password: e.get_password().unwrap_or("").to_string(),
            })
            .filter(|e| !e.password.is_empty())
            .collect();
        Ok(Vault { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn matches(&self, page_url: &str) -> Vec<usize> {
        let mut hits: Vec<(u32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| score(&e.url, page_url).map(|s| (s, i)))
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        hits.into_iter().map(|(_, i)| i).collect()
    }

    pub fn label(&self, i: usize) -> String {
        let e = &self.entries[i];
        if e.username.is_empty() {
            e.title.clone()
        } else {
            format!("{} · {}", e.title, e.username)
        }
    }

    pub fn get(&self, i: usize) -> Option<(&str, &str)> {
        self.entries.get(i).map(|e| (e.username.as_str(), e.password.as_str()))
    }
}

pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.contains(char::is_whitespace) {
        return String::new();
    }
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

fn score(entry_url: &str, page_url: &str) -> Option<u32> {
    let e = host_of(entry_url);
    let p = host_of(page_url);
    if e.is_empty() || p.is_empty() {
        return None;
    }
    let local = p == "localhost" || p == "127.0.0.1";
    if !page_url.trim().to_ascii_lowercase().starts_with("https://") && !local {
        return None;
    }
    if e == p {
        return Some(2);
    }
    if p.ends_with(&format!(".{e}")) || e.ends_with(&format!(".{p}")) {
        return Some(1);
    }
    None
}

#[allow(dead_code)]
pub fn host_matches(entry_url: &str, page_url: &str) -> bool {
    score(entry_url, page_url).is_some()
}

pub fn fill_js(username: &str, password: &str) -> String {
    let (u, p) = (js_str(username), js_str(password));
    format!(
        r#"( ()=>{{
  const set=(el,v)=>{{
    const s=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
    s.call(el,v);
    el.dispatchEvent(new Event('input',{{bubbles:true}}));
    el.dispatchEvent(new Event('change',{{bubbles:true}}));
  }};
  try{{
    const act=document.activeElement;
    const pwd=(act&&act.tagName==='INPUT'&&act.type==='password')?act:document.querySelector('input[type=password]:not([disabled]):not([readonly])');
    if(!pwd) return false;
    const u={u}, p={p};
    if(u){{
      const form=pwd.form;
      let user=null;
      if(form){{
        const list=[...form.querySelectorAll('input')];
        for(let k=list.indexOf(pwd)-1;k>=0;k--){{
          const t=list[k];
          if(t!==pwd&&(t.type==='text'||t.type==='email'||t.type==='tel')&&!t.disabled){{ user=t; break; }}
        }}
      }}
      if(user) set(user,u);
    }}
    set(pwd,p);
    return true;
  }}catch(e){{ return false; }}
}})()"#
    )
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_plain_and_lower() {
        assert_eq!(host_of("https://www.Example.com:8443/a/b?c#d"), "example.com");
        assert_eq!(host_of("https://user:pass@login.example.com/x"), "login.example.com");
        assert_eq!(host_of("example.com/x"), "example.com");
        assert_eq!(host_of("not a url"), "");
    }

    #[test]
    fn entry_matches_its_site_and_its_subdomains() {
        assert!(host_matches("https://example.com/login", "https://example.com/"));
        assert!(host_matches("https://example.com/", "https://login.example.com/"));
        assert!(host_matches("https://login.example.com/", "https://example.com/"));
        assert!(!host_matches("https://example.com/", "https://example.org/"));
        assert!(!host_matches("", "https://example.com/"));
        assert!(!host_matches("https://example.com/", ""));
        assert!(!host_matches("https://ample.com/", "https://example.com/"));
        assert!(host_matches("example.com/login", "https://www.example.com/"));
        assert!(!host_matches("example.com/login", "https://example.org/"));
    }

    #[test]
    fn exact_host_sorts_before_a_parent_domain() {
        let v = Vault {
            entries: vec![
                Entry {
                    title: "parent".into(),
                    username: "u".into(),
                    url: "https://example.com".into(),
                    password: "p".into(),
                },
                Entry {
                    title: "exact".into(),
                    username: "u".into(),
                    url: "https://login.example.com".into(),
                    password: "p".into(),
                },
                Entry {
                    title: "other".into(),
                    username: "u".into(),
                    url: "https://example.org".into(),
                    password: "p".into(),
                },
            ],
        };
        let hits = v.matches("https://login.example.com/home");
        assert_eq!(hits, vec![1, 0]);
    }

    #[test]
    fn fill_script_carries_the_secrets_as_json() {
        let js = fill_js("ann\"ex@x.com", "p'a\\ss</script>");
        assert!(js.contains(r#"ann\"ex@x.com"#));
        assert!(js.contains(r#"p'a\\ss</script>"#));
        assert!(js.contains("HTMLInputElement.prototype"));
        assert!(js.contains("type=password"));
    }

    #[test]
    fn a_real_kdbx4_file_opens_and_its_entries_come_out() {
        use keepass::{
            Database, DatabaseKey,
            config::{DatabaseConfig, KdfConfig},
            db::fields,
        };

        let mut cfg = DatabaseConfig::default();
        cfg.kdf_config = KdfConfig::Aes { rounds: 600 };
        let mut db = Database::with_config(cfg);
        {
            let mut root = db.root_mut();
            let mut e = root.add_entry();
            e.set_unprotected(fields::TITLE, "Example");
            e.set_unprotected(fields::USERNAME, "ann@example.com");
            e.set_protected(fields::PASSWORD, "s3cret");
            e.set_unprotected(fields::URL, "https://example.com/login");
        }
        {
            let mut root = db.root_mut();
            let mut e = root.add_entry();
            e.set_unprotected(fields::TITLE, "Notes only");
            e.set_unprotected(fields::URL, "https://example.org/");
        }
        let path = std::env::temp_dir().join(format!("lumen-kdbx-{}.kdbx", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).expect("a temp file to write");
            db.save(&mut f, DatabaseKey::new().with_password("master")).expect("the database saves");
        }

        let v = Vault::open(&path, "master").expect("the file opens");
        assert_eq!(v.len(), 1);
        assert_eq!(v.get(0), Some(("ann@example.com", "s3cret")));
        assert_eq!(v.label(0), "Example · ann@example.com");
        assert_eq!(v.matches("https://example.com/somewhere-else"), vec![0]);

        assert!(Vault::open(&path, "wrong").is_err());
        assert!(Vault::open(Path::new("does-not-exist.kdbx"), "master").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn never_offered_on_plain_http() {
        assert!(!host_matches("https://example.com", "http://example.com/login"));
        assert!(host_matches("https://example.com", "https://example.com/login"));
        assert!(host_matches("http://localhost:3000", "http://localhost:3000/"));
    }
}
