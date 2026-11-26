use anyhow::Context;
use reqwest::blocking::Client;
use reqwest::blocking::RequestBuilder;
use reqwest::blocking::Response;
use serde::Deserialize;
use serde_json::json;

pub struct Repo {
  api: String,
  token: Option<String>,
  client: Client,
}

#[derive(Deserialize)]
struct RepoInfo {
  default_branch: String,
}

#[derive(Deserialize)]
struct Pull {
  number: u64,
  head: PullBranch,
}

#[derive(Deserialize)]
struct PullBranch {
  #[serde(rename = "ref")]
  name: String,
}

impl Repo {
  /// Derives the Gitea API endpoint from a remote URL
  /// (`https://host[:port][/subpath]/owner/repo.git`, `ssh://git@host[:port]/owner/repo.git` or `git@host:owner/repo.git`).
  pub fn from_remote(
    remote: &str,
    token: Option<String>,
  ) -> anyhow::Result<Self> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let (scheme, rest) = match remote.split_once("://") {
      Some((scheme, rest)) => (scheme, rest.to_string()),
      None => ("ssh", remote.replacen(':', "/", 1)),
    };
    let (authority, path) = rest
      .split_once('/')
      .with_context(|| format!("unrecognised remote URL '{remote}'"))?;
    let host = authority
      .rsplit_once('@')
      .map_or(authority, |(_, host)| host);
    let origin = match scheme {
      "http" | "https" => format!("{scheme}://{host}"),
      // shortcut: assumes the web UI is served over https on the SSH host, add a URL override if that fails
      _ => format!(
        "https://{}",
        host.split_once(':').map_or(host, |(host, _)| host)
      ),
    };

    let mut segments = path.trim_matches('/').rsplitn(3, '/');
    let (Some(repo), Some(owner)) = (segments.next(), segments.next()) else {
      anyhow::bail!("remote URL '{remote}' has no owner/repo path");
    };
    let prefix = segments.next().map_or(String::new(), |p| format!("/{p}"));

    Ok(Self {
      api: format!("{origin}{prefix}/api/v1/repos/{owner}/{repo}"),
      token,
      client: Client::new(),
    })
  }

  pub fn default_branch(&self) -> anyhow::Result<String> {
    let info: RepoInfo = self.send(self.client.get(&self.api))?.json()?;
    Ok(info.default_branch)
  }

  pub fn find_pull_request(
    &self,
    branch: &str,
  ) -> anyhow::Result<Option<u64>> {
    for page in 1..=u32::MAX {
      let url = format!("{}/pulls?state=open&limit=50&page={page}", self.api);
      let pulls: Vec<Pull> = self.send(self.client.get(url))?.json()?;
      if pulls.is_empty() {
        break;
      }
      if let Some(pull) = pulls.iter().find(|pull| pull.head.name == branch) {
        return Ok(Some(pull.number));
      }
    }
    Ok(None)
  }

  pub fn create_pull_request(
    &self,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
  ) -> anyhow::Result<()> {
    // Gitea has no draft flag; a `WIP:` title prefix marks a PR as a draft.
    let title = match draft {
      true => format!("WIP: {title}"),
      false => title.to_string(),
    };
    let payload = json!({ "head": head, "base": base, "title": title, "body": body });
    self.send(
      self
        .client
        .post(format!("{}/pulls", self.api))
        .json(&payload),
    )?;
    Ok(())
  }

  pub fn edit_pull_request(
    &self,
    number: u64,
    base: &str,
    title: &str,
    body: &str,
  ) -> anyhow::Result<()> {
    let payload = json!({ "base": base, "title": title, "body": body });
    let url = format!("{}/pulls/{number}", self.api);
    self.send(self.client.patch(url).json(&payload))?;
    Ok(())
  }

  fn send(
    &self,
    request: RequestBuilder,
  ) -> anyhow::Result<Response> {
    let request = match &self.token {
      Some(token) => request.header("Authorization", format!("token {token}")),
      None => request,
    };
    let response = request.send().context("Gitea API request failed")?;
    let status = response.status();
    if !status.is_success() {
      let body = response.text().unwrap_or_default();
      anyhow::bail!("Gitea API request failed (status {status}): {body}");
    }
    Ok(response)
  }
}
