use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

use crate::cluster_state::{ClusterSecrets, ClusterState};

const BASE_DN: &str = "dc=azcluster,dc=local";
const ADMIN_DN: &str = "cn=admin,dc=azcluster,dc=local";
const DEFAULT_GID: u32 = 20000;

pub struct NewUser<'a> {
    pub username: &'a str,
    pub uid: u32,
    pub gid: u32,
    pub gecos: &'a str,
    pub shell: &'a str,
    pub ssh_keys: &'a [String],
}

pub fn render_user_add_ldif(u: &NewUser<'_>) -> String {
    let mut s = String::new();
    s.push_str(&format!("dn: uid={},ou=people,{}\n", u.username, BASE_DN));
    s.push_str("objectClass: top\n");
    s.push_str("objectClass: account\n");
    s.push_str("objectClass: posixAccount\n");
    s.push_str("objectClass: shadowAccount\n");
    s.push_str("objectClass: ldapPublicKey\n");
    s.push_str(&format!("uid: {}\n", u.username));
    s.push_str(&format!("cn: {}\n", u.username));
    s.push_str(&format!("uidNumber: {}\n", u.uid));
    s.push_str(&format!("gidNumber: {}\n", u.gid));
    s.push_str(&format!("homeDirectory: /shared/home/{}\n", u.username));
    s.push_str(&format!("loginShell: {}\n", u.shell));
    s.push_str(&format!("gecos: {}\n", u.gecos));
    for k in u.ssh_keys {
        s.push_str(&format!("sshPublicKey: {}\n", k.trim()));
    }
    s
}

pub fn render_user_delete_ldif(username: &str) -> String {
    format!(
        "dn: uid={},ou=people,{}\nchangetype: delete\n",
        username, BASE_DN
    )
}

pub fn render_sshkey_add_ldif(username: &str, key: &str) -> String {
    format!(
        "dn: uid={},ou=people,{}\nchangetype: modify\nadd: sshPublicKey\nsshPublicKey: {}\n",
        username,
        BASE_DN,
        key.trim()
    )
}

pub fn render_sshkey_remove_ldif(username: &str, key: &str) -> String {
    format!(
        "dn: uid={},ou=people,{}\nchangetype: modify\ndelete: sshPublicKey\nsshPublicKey: {}\n",
        username,
        BASE_DN,
        key.trim()
    )
}

pub fn render_uid_bump_ldif(current_uid: u32) -> String {
    format!(
        "dn: cn=uidNext,{}\nchangetype: modify\nreplace: uidNumber\nuidNumber: {}\n",
        BASE_DN,
        current_uid + 1
    )
}

pub fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 32 {
        return Err(anyhow!("username must be 1-32 chars"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(anyhow!(
            "username may contain only [a-z0-9_-], got '{}'",
            name
        ));
    }
    if !name.chars().next().unwrap().is_ascii_lowercase() {
        return Err(anyhow!("username must start with [a-z]"));
    }
    Ok(())
}

fn ssh_to_scheduler_with_stdin(
    state: &ClusterState,
    remote_cmd: &str,
    stdin_data: &str,
) -> Result<String> {
    let host = state.login_public_ip.as_deref().ok_or_else(|| {
        anyhow!(
            "cluster '{}' has no login public IP. Redeploy with --login-public-ip.",
            state.name
        )
    })?;
    let login_target = format!("{}@{}", state.admin_username, host);
    let sched_target = format!("{}@{}", state.admin_username, state.scheduler_private_ip);
    let mut child = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-J",
            &login_target,
            &sched_target,
            "--",
            "bash",
            "-lc",
            remote_cmd,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn ssh -J")?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .context("write stdin")?;
    let out = child.wait_with_output().context("wait ssh")?;
    if !out.status.success() {
        return Err(anyhow!("ssh exec failed: status={:?}", out.status.code()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn read_ldap_password(cluster_name: &str) -> Result<String> {
    Ok(ClusterSecrets::load(cluster_name)?.ldap_admin_password)
}

fn fetch_next_uid(state: &ClusterState, password: &str) -> Result<u32> {
    let cmd = format!(
        "ldapsearch -x -LLL -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1 -b 'cn=uidNext,{}' -s base uidNumber",
        ADMIN_DN, BASE_DN
    );
    let wrapped = format!("LDAP_PW=$(cat); export LDAP_PW; {}", cmd);
    let out = ssh_to_scheduler_with_stdin(state, &wrapped, password)?;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("uidNumber:") {
            return rest.trim().parse::<u32>().context("parse uidNumber");
        }
    }
    Err(anyhow!(
        "could not find uidNumber in ldapsearch output: {}",
        out
    ))
}

pub fn user_add(
    state: &ClusterState,
    username: &str,
    explicit_uid: Option<u32>,
    explicit_gid: Option<u32>,
    gecos: &str,
    shell: &str,
    ssh_key_files: &[std::path::PathBuf],
) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let mut keys: Vec<String> = Vec::new();
    for p in ssh_key_files {
        let k =
            std::fs::read_to_string(p).with_context(|| format!("read ssh key {}", p.display()))?;
        for line in k.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                keys.push(line.to_string());
            }
        }
    }
    let uid = match explicit_uid {
        Some(u) => u,
        None => fetch_next_uid(state, &password)?,
    };
    let gid = explicit_gid.unwrap_or(DEFAULT_GID);
    let user = NewUser {
        username,
        uid,
        gid,
        gecos,
        shell,
        ssh_keys: &keys,
    };
    let mut ldif = render_user_add_ldif(&user);
    if explicit_uid.is_none() {
        ldif.push('\n');
        ldif.push_str(&render_uid_bump_ldif(uid));
    }
    let remote = format!(
        "LDAP_PW=$(head -c 1024); LDIF=$(cat); echo \"$LDIF\" | ldapadd -x -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1 -c",
        ADMIN_DN
    );
    let stdin_payload = format!("{}\n{}", password, ldif);
    ssh_to_scheduler_with_stdin(state, &remote, &stdin_payload)?;
    eprintln!("==> added user '{}' (uid={}, gid={})", username, uid, gid);
    Ok(())
}

pub fn user_remove(state: &ClusterState, username: &str) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let ldif = render_user_delete_ldif(username);
    let remote = format!(
        "LDAP_PW=$(head -c 1024); LDIF=$(cat); echo \"$LDIF\" | ldapmodify -x -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1",
        ADMIN_DN
    );
    let stdin_payload = format!("{}\n{}", password, ldif);
    ssh_to_scheduler_with_stdin(state, &remote, &stdin_payload)?;
    eprintln!("==> removed user '{}'", username);
    Ok(())
}

pub fn user_list(state: &ClusterState) -> Result<()> {
    let password = read_ldap_password(&state.name)?;
    let cmd = format!(
        "LDAP_PW=$(cat); ldapsearch -x -LLL -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1 -b 'ou=people,{}' '(objectClass=posixAccount)' uid uidNumber gidNumber loginShell gecos",
        ADMIN_DN, BASE_DN
    );
    let out = ssh_to_scheduler_with_stdin(state, &cmd, &password)?;
    print!("{}", out);
    Ok(())
}

pub fn sshkey_add(state: &ClusterState, username: &str, key_file: &std::path::Path) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let raw = std::fs::read_to_string(key_file)
        .with_context(|| format!("read ssh key {}", key_file.display()))?;
    let key = raw
        .lines()
        .find(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#')
        })
        .ok_or_else(|| anyhow!("no usable key line in {}", key_file.display()))?
        .trim()
        .to_string();
    let ldif = render_sshkey_add_ldif(username, &key);
    let remote = format!(
        "LDAP_PW=$(head -c 1024); LDIF=$(cat); echo \"$LDIF\" | ldapmodify -x -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1",
        ADMIN_DN
    );
    let payload = format!("{}\n{}", password, ldif);
    ssh_to_scheduler_with_stdin(state, &remote, &payload)?;
    eprintln!("==> added ssh key for '{}'", username);
    Ok(())
}

pub fn sshkey_remove(
    state: &ClusterState,
    username: &str,
    key_file: &std::path::Path,
) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let raw = std::fs::read_to_string(key_file)
        .with_context(|| format!("read ssh key {}", key_file.display()))?;
    let key = raw
        .lines()
        .find(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#')
        })
        .ok_or_else(|| anyhow!("no usable key line in {}", key_file.display()))?
        .trim()
        .to_string();
    let ldif = render_sshkey_remove_ldif(username, &key);
    let remote = format!(
        "LDAP_PW=$(head -c 1024); LDIF=$(cat); echo \"$LDIF\" | ldapmodify -x -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1",
        ADMIN_DN
    );
    let payload = format!("{}\n{}", password, ldif);
    ssh_to_scheduler_with_stdin(state, &remote, &payload)?;
    eprintln!("==> removed ssh key for '{}'", username);
    Ok(())
}

pub fn sshkey_list(state: &ClusterState, username: &str) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let cmd = format!(
        "LDAP_PW=$(cat); ldapsearch -x -LLL -D '{}' -w \"$LDAP_PW\" -H ldap://127.0.0.1 -b 'uid={},ou=people,{}' -s base sshPublicKey",
        ADMIN_DN, username, BASE_DN
    );
    let out = ssh_to_scheduler_with_stdin(state, &cmd, &password)?;
    print!("{}", out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_ldif_has_required_fields() {
        let keys = vec!["ssh-ed25519 AAAA test".to_string()];
        let u = NewUser {
            username: "alice",
            uid: 20005,
            gid: 20000,
            gecos: "Alice",
            shell: "/bin/bash",
            ssh_keys: &keys,
        };
        let ldif = render_user_add_ldif(&u);
        assert!(ldif.contains("dn: uid=alice,ou=people,dc=azcluster,dc=local"));
        assert!(ldif.contains("uidNumber: 20005"));
        assert!(ldif.contains("gidNumber: 20000"));
        assert!(ldif.contains("homeDirectory: /shared/home/alice"));
        assert!(ldif.contains("loginShell: /bin/bash"));
        assert!(ldif.contains("sshPublicKey: ssh-ed25519 AAAA test"));
        assert!(ldif.contains("objectClass: ldapPublicKey"));
        assert!(ldif.contains("objectClass: posixAccount"));
    }

    #[test]
    fn add_ldif_omits_key_when_empty() {
        let u = NewUser {
            username: "bob",
            uid: 20006,
            gid: 20000,
            gecos: "Bob",
            shell: "/bin/bash",
            ssh_keys: &[],
        };
        let ldif = render_user_add_ldif(&u);
        assert!(!ldif.contains("sshPublicKey:"));
    }

    #[test]
    fn delete_ldif_shape() {
        let ldif = render_user_delete_ldif("carol");
        assert_eq!(
            ldif,
            "dn: uid=carol,ou=people,dc=azcluster,dc=local\nchangetype: delete\n"
        );
    }

    #[test]
    fn sshkey_add_ldif_modify_op() {
        let ldif = render_sshkey_add_ldif("dave", "ssh-rsa BBB dave@laptop\n");
        assert!(ldif.contains("changetype: modify"));
        assert!(ldif.contains("add: sshPublicKey"));
        assert!(ldif.contains("sshPublicKey: ssh-rsa BBB dave@laptop"));
        assert!(!ldif.contains("dave@laptop\n\n"));
    }

    #[test]
    fn sshkey_remove_ldif_modify_op() {
        let ldif = render_sshkey_remove_ldif("eve", "ssh-ed25519 CCC eve");
        assert!(ldif.contains("changetype: modify"));
        assert!(ldif.contains("delete: sshPublicKey"));
    }

    #[test]
    fn uid_bump_increments_by_one() {
        let ldif = render_uid_bump_ldif(20007);
        assert!(ldif.contains("dn: cn=uidNext,dc=azcluster,dc=local"));
        assert!(ldif.contains("replace: uidNumber"));
        assert!(ldif.contains("uidNumber: 20008"));
    }

    #[test]
    fn username_accepts_valid() {
        for n in &["alice", "bob_1", "u-2", "a1b2"] {
            assert!(validate_username(n).is_ok(), "rejected: {}", n);
        }
    }

    #[test]
    fn username_rejects_invalid() {
        let too_long = "x".repeat(33);
        for n in &["", "Alice", "1alice", "alice!", too_long.as_str()] {
            assert!(validate_username(n).is_err(), "accepted: {}", n);
        }
    }
}
