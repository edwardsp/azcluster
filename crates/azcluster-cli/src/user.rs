use anyhow::{anyhow, Context, Result};
use std::process::Command;

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

pub fn render_admin_grant_ldif(username: &str) -> String {
    format!(
        "dn: cn=cluster-admins,ou=groups,{}\nchangetype: modify\nadd: memberUid\nmemberUid: {}\n",
        BASE_DN, username
    )
}

pub fn render_admin_revoke_ldif(username: &str) -> String {
    format!(
        "dn: cn=cluster-admins,ou=groups,{}\nchangetype: modify\ndelete: memberUid\nmemberUid: {}\n",
        BASE_DN, username
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

pub fn b64_encode(data: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

fn ssh_run(state: &ClusterState, remote_cmd: &str) -> Result<String> {
    let use_bastion = crate::should_use_bastion(state, false);
    let identity = crate::resolve_identity(None, &state.name)
        .with_context(|| format!("resolve admin ssh identity for '{}'", state.name))?;
    let sched_target = format!("{}@{}", state.admin_username, state.scheduler_private_ip);
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "BatchMode=yes",
        "-o",
        "IdentitiesOnly=yes",
        "-i",
        &identity.display().to_string(),
    ]);
    if use_bastion {
        let exe = crate::self_exe_path()?;
        let proxy = format!(
            "{} bastion-proxy --cluster {} --target scheduler",
            exe, state.name
        );
        cmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
        cmd.args([&sched_target, "--", "bash", "-lc", remote_cmd]);
    } else {
        let host = state.login_public_ip.as_deref().ok_or_else(|| {
            anyhow!(
                "cluster '{}' has no login public IP and bastion is not enabled. \
                 Redeploy with --login-public-ip or --bastion.",
                state.name
            )
        })?;
        let login_target = format!("{}@{}", state.admin_username, host);
        crate::add_ssh_jump_with_identity(&mut cmd, &identity, &login_target);
        cmd.args([&sched_target, "--", "bash", "-lc", remote_cmd]);
    }
    let out = cmd.output().context("spawn ssh")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "ssh exec failed: status={:?} stderr={}",
            out.status.code(),
            stderr
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn build_sssd_flush_cmd(username: &str) -> String {
    format!(
        "sudo -n sss_cache -u '{u}' >/dev/null 2>&1 || true; \
         sudo -n sss_cache -E >/dev/null 2>&1 || true",
        u = username,
    )
}

fn flush_login_sssd_cache(state: &ClusterState, username: &str) {
    let use_bastion = crate::should_use_bastion(state, false);
    let identity = match crate::resolve_identity(None, &state.name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "==> warn: SSSD cache flush skipped (could not resolve admin ssh key: {}); key changes will propagate within ~60s via entry_cache_timeout",
                e
            );
            return;
        }
    };
    let host = if use_bastion {
        "127.0.0.1".to_string()
    } else {
        match state.login_public_ip.as_deref() {
            Some(h) => h.to_string(),
            None => return,
        }
    };
    let target = format!("{}@{}", state.admin_username, host);
    let cmd = build_sssd_flush_cmd(username);
    let mut sshcmd = Command::new("ssh");
    sshcmd.args([
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "IdentitiesOnly=yes",
        "-i",
        &identity.display().to_string(),
    ]);
    if use_bastion {
        if let Ok(exe) = crate::self_exe_path() {
            let proxy = format!(
                "{} bastion-proxy --cluster {} --target login",
                exe, state.name
            );
            sshcmd.args(["-o", &format!("ProxyCommand={}", proxy)]);
        }
    }
    sshcmd.args([&target, "--", "bash", "-lc", &cmd]);
    let out = sshcmd.output();
    match out {
        Ok(o) if o.status.success() => {
            eprintln!("==> flushed SSSD cache on login for '{}'", username);
        }
        Ok(o) => {
            eprintln!(
                "==> warn: SSSD cache flush on login returned status={:?} (key changes will propagate within ~60s via entry_cache_timeout)",
                o.status.code()
            );
        }
        Err(e) => {
            eprintln!(
                "==> warn: could not ssh to login to flush SSSD cache ({}); key changes will propagate within ~60s via entry_cache_timeout",
                e
            );
        }
    }
}

// Wraps sacctmgr in a retry+classify shell helper.
// Retries on transient errors ("Connection refused", "cluster has not been
// added") because slurmdbd can take a few minutes to come up after
// `azcluster deploy` reports success. Idempotent on "Already existing",
// "Nothing new added", "Nothing deleted". Surfaces any other failure with
// a non-zero exit so ssh_run propagates an Err.
//
// `systemctl restart slurmctld` (not `scontrol reconfigure`): slurmctld's
// assoc_mgr caches the username->uid mapping for the process lifetime.
// `scontrol reconfigure` re-reads slurm.conf only; it does not re-resolve
// cached uids. Without the restart, a remove+re-add of the same username
// with a fresh uid leaves slurmctld dispatching against the stale uid and
// sbatch fails with "Invalid account or account/partition combination".
// Restart cost: ~3 s, zero running-job impact (slurmd retains workload).
const SACCTMGR_RETRY_HELPER: &str = r#"
set -e
sacctmgr_run() {
  local label="$1"; shift
  local out rc i
  for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
    set +e
    out=$(sudo -n sacctmgr -i "$@" 2>&1)
    rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then return 0; fi
    case "$out" in
      *"already exists"*|*"Already existing"*|*"Nothing new added"*|*"Nothing deleted"*) return 0 ;;
      *"Connection refused"*|*"Unable to contact slurmdbd"*|*"slurmdbd"*[Dd]"own"*|*"cluster has not been added"*|*"is not registered"*)
        sleep 5; continue ;;
    esac
    echo "sacctmgr $label FAILED rc=$rc: $out" >&2
    return $rc
  done
  echo "sacctmgr $label transient after 60s, last: $out" >&2
  return 1
}
"#;

fn build_sacctmgr_add_cmd(username: &str) -> String {
    format!(
        "{helper}\n\
         sacctmgr_run 'add account {u}' add account '{u}' \
           Description='azcluster user {u}' Organization=azcluster\n\
         sacctmgr_run 'add user {u}' add user '{u}' DefaultAccount='{u}'\n\
         # Add a per-partition association so AccountingStorageEnforce=associations\n\
         # accepts jobs from this user on every existing partition. Without this,\n\
         # sbatch returns 'Invalid account or account/partition combination'.\n\
         for p in $(sinfo -h -o '%R' 2>/dev/null | sort -u); do\n\
           sacctmgr_run \"associate {u} <-> $p\" add user '{u}' Account='{u}' Partition=\"$p\"\n\
         done\n\
         sudo -n sss_cache -u '{u}' >/dev/null 2>&1 || true\n\
         sudo -n sss_cache -E >/dev/null 2>&1 || true\n\
         sudo -n systemctl restart slurmctld >/dev/null 2>&1 || true\n\
         sleep 3\n",
        helper = SACCTMGR_RETRY_HELPER,
        u = username,
    )
}

fn build_sacctmgr_remove_cmd(username: &str) -> String {
    format!(
        "{helper}\n\
         sacctmgr_run 'delete user {u}' delete user name='{u}'\n\
         sacctmgr_run 'delete account {u}' delete account name='{u}'\n\
         sudo -n sss_cache -u '{u}' >/dev/null 2>&1 || true\n\
         sudo -n sss_cache -E >/dev/null 2>&1 || true\n\
         sudo -n systemctl restart slurmctld >/dev/null 2>&1 || true\n\
         sleep 3\n",
        helper = SACCTMGR_RETRY_HELPER,
        u = username,
    )
}

fn register_slurm_account(state: &ClusterState, username: &str) {
    let cmd = build_sacctmgr_add_cmd(username);
    match ssh_run(state, &cmd) {
        Ok(_) => eprintln!(
            "==> registered '{}' with Slurm accounting (account='{}', DefaultAccount='{}')",
            username, username, username
        ),
        Err(e) => {
            eprintln!("==> warn: sacctmgr add for '{}' failed: {}", username, e);
            eprintln!(
                "         run on scheduler: sudo sacctmgr -i add account {u} && sudo sacctmgr -i add user {u} DefaultAccount={u}",
                u = username
            );
        }
    }
}

fn deregister_slurm_account(state: &ClusterState, username: &str) {
    let cmd = build_sacctmgr_remove_cmd(username);
    match ssh_run(state, &cmd) {
        Ok(_) => eprintln!("==> deregistered '{}' from Slurm accounting", username),
        Err(e) => {
            eprintln!("==> warn: sacctmgr delete for '{}' failed: {}", username, e);
            eprintln!(
                "         run on scheduler: sudo sacctmgr -i delete user name={u} && sudo sacctmgr -i delete account name={u}",
                u = username
            );
        }
    }
}

fn build_ldap_write_cmd(password: &str, ldif: &str, tool: &str, extra_args: &str) -> String {
    let pw_b64 = b64_encode(password.as_bytes());
    let ldif_b64 = b64_encode(ldif.as_bytes());
    format!(
        "set -e; \
         PW=$(printf %s '{pw_b64}' | base64 -d); \
         LDIF=$(printf %s '{ldif_b64}' | base64 -d); \
         printf %s \"$LDIF\" | {tool} -x -D '{admin}' -w \"$PW\" -H ldap://127.0.0.1 {extra}",
        pw_b64 = pw_b64,
        ldif_b64 = ldif_b64,
        tool = tool,
        admin = ADMIN_DN,
        extra = extra_args,
    )
}

fn build_admin_set_cmd(password: &str, username: &str, grant: bool) -> String {
    let ldif = if grant {
        render_admin_grant_ldif(username)
    } else {
        render_admin_revoke_ldif(username)
    };
    build_ldap_write_cmd(password, &ldif, "ldapmodify", "-c")
}

fn build_ldap_search_cmd(
    password: &str,
    base: &str,
    scope: &str,
    filter: &str,
    attrs: &str,
) -> String {
    let pw_b64 = b64_encode(password.as_bytes());
    format!(
        "set -e; \
         PW=$(printf %s '{pw_b64}' | base64 -d); \
         ldapsearch -x -LLL -D '{admin}' -w \"$PW\" -H ldap://127.0.0.1 -b '{base}' -s {scope} '{filter}' {attrs}",
        pw_b64 = pw_b64,
        admin = ADMIN_DN,
        base = base,
        scope = scope,
        filter = filter,
        attrs = attrs,
    )
}

fn read_ldap_password(cluster_name: &str) -> Result<String> {
    Ok(ClusterSecrets::load(cluster_name)?.ldap_admin_password)
}

fn fetch_next_uid(state: &ClusterState, password: &str) -> Result<u32> {
    let cmd = build_ldap_search_cmd(
        password,
        &format!("cn=uidNext,{}", BASE_DN),
        "base",
        "(objectClass=*)",
        "uidNumber",
    );
    let out = ssh_run(state, &cmd)?;
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

#[allow(clippy::too_many_arguments)]
pub fn user_add(
    state: &ClusterState,
    username: &str,
    explicit_uid: Option<u32>,
    explicit_gid: Option<u32>,
    gecos: &str,
    shell: &str,
    ssh_key_files: &[std::path::PathBuf],
    admin: bool,
    generate_keypair: bool,
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

    if generate_keypair {
        let comment = format!("azcluster-{}-{}", state.name, username);
        let kp = crate::crypto::generate_admin_keypair(&comment)
            .with_context(|| format!("generate ssh keypair for user '{}'", username))?;
        let key_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("could not determine HOME"))?
            .join(".azcluster")
            .join("keys");
        std::fs::create_dir_all(&key_dir)
            .with_context(|| format!("mkdir {}", key_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700)).ok();
        }
        let priv_path = key_dir.join(format!("{}-{}", state.name, username));
        std::fs::write(&priv_path, &kp.private_openssh_pem)
            .with_context(|| format!("write {}", priv_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", priv_path.display()))?;
        }
        eprintln!(
            "==> generated user keypair -> {} (private, 0600)",
            priv_path.display()
        );
        keys.push(kp.public_openssh.trim().to_string());
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
    let cmd = build_ldap_write_cmd(&password, &ldif, "ldapadd", "-c");
    ssh_run(state, &cmd)?;
    eprintln!("==> added user '{}' (uid={}, gid={})", username, uid, gid);

    if admin {
        let cmd = build_admin_set_cmd(&password, username, true);
        ssh_run(state, &cmd).context("add user to cluster-admins group")?;
        eprintln!(
            "==> granted admin privileges (member of cn=cluster-admins) to '{}'",
            username
        );
    }

    flush_login_sssd_cache(state, username);
    if state.accounting_enabled {
        register_slurm_account(state, username);
    }
    Ok(())
}

pub fn user_setadmin(state: &ClusterState, username: &str, grant: bool) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let cmd = build_admin_set_cmd(&password, username, grant);
    ssh_run(state, &cmd).context("modify cn=cluster-admins membership")?;
    eprintln!(
        "==> {} '{}' {} cn=cluster-admins",
        if grant { "added" } else { "removed" },
        username,
        if grant { "to" } else { "from" },
    );
    flush_login_sssd_cache(state, username);
    Ok(())
}

pub fn user_remove(state: &ClusterState, username: &str) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let ldif = render_user_delete_ldif(username);
    let cmd = build_ldap_write_cmd(&password, &ldif, "ldapmodify", "");
    ssh_run(state, &cmd)?;
    eprintln!("==> removed user '{}'", username);
    flush_login_sssd_cache(state, username);
    if state.accounting_enabled {
        deregister_slurm_account(state, username);
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct LdapUserRow {
    uid: String,
    uid_number: String,
    gid_number: String,
    shell: String,
    gecos: String,
}

fn parse_ldif_user_rows(ldif: &str) -> Vec<LdapUserRow> {
    let mut rows: Vec<LdapUserRow> = Vec::new();
    let mut cur = LdapUserRow::default();
    let mut started = false;
    let flush = |cur: &mut LdapUserRow, started: &mut bool, rows: &mut Vec<LdapUserRow>| {
        if *started && !cur.uid.is_empty() {
            rows.push(std::mem::take(cur));
        } else {
            *cur = LdapUserRow::default();
        }
        *started = false;
    };
    for raw in ldif.lines() {
        let line = raw.trim_end_matches(['\r']);
        if line.is_empty() {
            flush(&mut cur, &mut started, &mut rows);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim_start_matches(' ').to_string();
        started = true;
        match key {
            "uid" => cur.uid = value,
            "uidNumber" => cur.uid_number = value,
            "gidNumber" => cur.gid_number = value,
            "loginShell" => cur.shell = value,
            "gecos" => cur.gecos = value,
            _ => {}
        }
    }
    flush(&mut cur, &mut started, &mut rows);
    rows.sort_by(|a, b| {
        a.uid_number
            .parse::<u64>()
            .unwrap_or(u64::MAX)
            .cmp(&b.uid_number.parse::<u64>().unwrap_or(u64::MAX))
            .then_with(|| a.uid.cmp(&b.uid))
    });
    rows
}

pub fn user_list(state: &ClusterState) -> Result<()> {
    let password = read_ldap_password(&state.name)?;
    let cmd = build_ldap_search_cmd(
        &password,
        &format!("ou=people,{}", BASE_DN),
        "sub",
        "(objectClass=posixAccount)",
        "uid uidNumber gidNumber loginShell gecos",
    );
    let out = ssh_run(state, &cmd)?;
    let rows = parse_ldif_user_rows(&out);
    if rows.is_empty() {
        eprintln!("(no users)");
        return Ok(());
    }
    let admins_cmd = build_ldap_search_cmd(
        &password,
        &format!("cn=cluster-admins,ou=groups,{}", BASE_DN),
        "base",
        "(objectClass=posixGroup)",
        "memberUid",
    );
    let admins_out = ssh_run(state, &admins_cmd).unwrap_or_default();
    let admins: std::collections::BTreeSet<String> = admins_out
        .lines()
        .filter_map(|l| l.strip_prefix("memberUid: ").map(|s| s.trim().to_string()))
        .collect();
    print!("{}", render_user_table_with_admin(&rows, &admins));
    Ok(())
}

fn render_user_table_with_admin(
    rows: &[LdapUserRow],
    admins: &std::collections::BTreeSet<String>,
) -> String {
    let mut s = format!(
        "{:<24} {:>8} {:>8} {:<6} {:<24} {}\n",
        "USERNAME", "UID", "GID", "ADMIN", "SHELL", "GECOS"
    );
    for r in rows {
        let admin = if admins.contains(&r.uid) { "yes" } else { "" };
        s.push_str(&format!(
            "{:<24} {:>8} {:>8} {:<6} {:<24} {}\n",
            r.uid, r.uid_number, r.gid_number, admin, r.shell, r.gecos
        ));
    }
    s
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
    let cmd = build_ldap_write_cmd(&password, &ldif, "ldapmodify", "");
    ssh_run(state, &cmd)?;
    eprintln!("==> added ssh key for '{}'", username);
    flush_login_sssd_cache(state, username);
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
    let cmd = build_ldap_write_cmd(&password, &ldif, "ldapmodify", "");
    ssh_run(state, &cmd)?;
    eprintln!("==> removed ssh key for '{}'", username);
    flush_login_sssd_cache(state, username);
    Ok(())
}

pub fn sshkey_list(state: &ClusterState, username: &str) -> Result<()> {
    validate_username(username)?;
    let password = read_ldap_password(&state.name)?;
    let cmd = build_ldap_search_cmd(
        &password,
        &format!("uid={},ou=people,{}", username, BASE_DN),
        "base",
        "(objectClass=*)",
        "sshPublicKey",
    );
    let out = ssh_run(state, &cmd)?;
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

    #[test]
    fn b64_matches_rfc4648_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn build_write_cmd_inlines_b64_and_no_stdin() {
        let cmd = build_ldap_write_cmd("hunter2", "dn: uid=x,ou=people\n", "ldapadd", "-c");
        assert!(cmd.contains("base64 -d"));
        assert!(cmd.contains("ldapadd -x -D 'cn=admin,dc=azcluster,dc=local'"));
        assert!(cmd.contains(" -c"));
        assert!(cmd.contains(&b64_encode(b"hunter2")));
        assert!(cmd.contains(&b64_encode(b"dn: uid=x,ou=people\n")));
    }

    #[test]
    fn build_search_cmd_inlines_password_b64() {
        let cmd = build_ldap_search_cmd("pw", "ou=people,dc=x", "sub", "(uid=*)", "uid");
        assert!(cmd.contains("ldapsearch -x -LLL -D 'cn=admin,"));
        assert!(cmd.contains(&b64_encode(b"pw")));
        assert!(cmd.contains("-b 'ou=people,dc=x' -s sub '(uid=*)' uid"));
    }

    #[test]
    fn flush_cmd_uses_sudo_n_and_invalidates_user_and_full() {
        let cmd = build_sssd_flush_cmd("alice");
        assert!(cmd.contains("sudo -n sss_cache -u 'alice'"));
        assert!(cmd.contains("sudo -n sss_cache -E"));
        assert!(
            cmd.contains("|| true"),
            "must be best-effort so remote failure does not propagate"
        );
    }

    #[test]
    fn flush_cmd_quotes_username() {
        let cmd = build_sssd_flush_cmd("bob_user-1");
        assert!(cmd.contains("'bob_user-1'"));
    }

    #[test]
    fn sacctmgr_add_creates_account_then_user_with_self_default() {
        let cmd = build_sacctmgr_add_cmd("alice");
        assert!(cmd.contains("add account 'alice'"));
        assert!(cmd.contains("Organization=azcluster"));
        assert!(cmd.contains("add user 'alice' DefaultAccount='alice'"));
        let account_pos = cmd.find("add account 'alice'").expect("account present");
        let user_pos = cmd.find("add user 'alice'").expect("user present");
        assert!(
            account_pos < user_pos,
            "account creation must precede user creation: {cmd}"
        );
        assert!(cmd.contains("sacctmgr_run"));
        assert!(cmd.contains("sudo -n sacctmgr -i"));
        assert_eq!(
            cmd.matches("|| true").count(),
            3,
            "should only swallow scheduler-side sss_cache + slurmctld restart, never sacctmgr exit codes: {cmd}"
        );
        assert!(
            cmd.contains("already exists") && cmd.contains("Already existing"),
            "must treat duplicate as idempotent for both Slurm casings: {cmd}"
        );
        assert!(
            cmd.contains("Connection refused") && cmd.contains("cluster has not been added"),
            "must retry on transient slurmdbd errors: {cmd}"
        );
        assert!(
            cmd.contains("sudo -n sss_cache -u 'alice'") && cmd.contains("sudo -n sss_cache -E"),
            "must flush scheduler-side SSSD cache before slurmctld restart so getpwnam returns the new uid: {cmd}"
        );
        assert!(
            cmd.contains("systemctl restart slurmctld"),
            "must restart slurmctld to invalidate cached uid mapping after sacctmgr add: {cmd}"
        );
        let sss_pos = cmd
            .find("sudo -n sss_cache -u 'alice'")
            .expect("sss flush present");
        let restart_pos = cmd
            .find("systemctl restart slurmctld")
            .expect("restart present");
        assert!(
            sss_pos < restart_pos,
            "scheduler SSSD flush must precede slurmctld restart: {cmd}"
        );
    }

    #[test]
    fn sacctmgr_remove_drops_user_then_account() {
        let cmd = build_sacctmgr_remove_cmd("alice");
        assert!(cmd.contains("delete user name='alice'"));
        assert!(cmd.contains("delete account name='alice'"));
        let user_pos = cmd.find("delete user").expect("user present");
        let account_pos = cmd.find("delete account").expect("account present");
        assert!(
            user_pos < account_pos,
            "user deletion must precede account deletion (FK): {cmd}"
        );
        assert!(cmd.contains("sacctmgr_run"));
        assert_eq!(
            cmd.matches("|| true").count(),
            3,
            "should only swallow scheduler-side sss_cache + slurmctld restart, never sacctmgr exit codes: {cmd}"
        );
        assert!(
            cmd.contains("sudo -n sss_cache -u 'alice'") && cmd.contains("sudo -n sss_cache -E"),
            "must flush scheduler-side SSSD cache before slurmctld restart: {cmd}"
        );
        assert!(
            cmd.contains("systemctl restart slurmctld"),
            "must restart slurmctld to invalidate cached uid mapping after sacctmgr delete: {cmd}"
        );
    }

    #[test]
    fn ldif_user_rows_parse_two_users_sorted_by_uid() {
        let ldif = "\
dn: uid=bob,ou=people,dc=azcluster,dc=local
uid: bob
uidNumber: 20005
gidNumber: 20000
loginShell: /bin/bash
gecos: Bob B

dn: uid=alice,ou=people,dc=azcluster,dc=local
uid: alice
uidNumber: 20001
gidNumber: 20000
loginShell: /bin/zsh
gecos: Alice A
";
        let rows = parse_ldif_user_rows(ldif);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].uid, "alice");
        assert_eq!(rows[0].uid_number, "20001");
        assert_eq!(rows[0].shell, "/bin/zsh");
        assert_eq!(rows[1].uid, "bob");
        assert_eq!(rows[1].uid_number, "20005");
    }

    #[test]
    fn ldif_user_rows_empty_when_no_records() {
        assert!(parse_ldif_user_rows("").is_empty());
        assert!(parse_ldif_user_rows("\n\n").is_empty());
    }

    #[test]
    fn user_table_pads_columns_and_includes_header() {
        let rows = vec![
            LdapUserRow {
                uid: "alice".into(),
                uid_number: "20001".into(),
                gid_number: "20000".into(),
                shell: "/bin/bash".into(),
                gecos: "Alice".into(),
            },
            LdapUserRow {
                uid: "bobbington".into(),
                uid_number: "20002".into(),
                gid_number: "20000".into(),
                shell: "/bin/zsh".into(),
                gecos: "Bob".into(),
            },
        ];
        let admins = std::collections::BTreeSet::new();
        let table = render_user_table_with_admin(&rows, &admins);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("USERNAME"));
        assert!(lines[0].contains("UID"));
        assert!(lines[0].contains("GID"));
        assert!(lines[0].contains("ADMIN"));
        assert!(lines[0].contains("SHELL"));
        assert!(lines[0].contains("GECOS"));
        let user_col = lines[1].split_whitespace().next().unwrap();
        assert_eq!(user_col, "alice");
        let user_col_2 = lines[2].split_whitespace().next().unwrap();
        assert_eq!(user_col_2, "bobbington");
        assert!(lines[1].contains("20001"));
        assert!(lines[2].contains("20002"));
    }
}
