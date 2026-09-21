//! Who may connect to each of the daemon's three sockets (QI-BB-014).
//!
//! Every socket is private unless its own knob says otherwise; a shared
//! query socket does not open the control or ingest socket. The knobs:
//!
//! - `QUANTA_INDEX_QUERY_SOCKET_ACCESS`
//! - `QUANTA_INDEX_CONTROL_SOCKET_ACCESS`
//! - `QUANTA_INDEX_INGEST_SOCKET_ACCESS`
//!
//! each either `private` (the default when unset) or
//! `shared:<item>[,<item>...]` where an item is `group=<name-or-gid>` (at
//! most once) or `uid=<name-or-uid>` (any number of times, no repeats). A
//! numeric value is taken as the id itself; anything else is looked up in
//! the system's user and group databases at boot, and a name that does not
//! resolve refuses boot before any socket is bound. `shared:` with no item
//! is refused too: it would admit nobody but the owner, which `private`
//! says honestly. The semantics of the resulting policy — file modes, the
//! effective-gid check, the traversal requirement on the socket's path —
//! are the IPC crate's ([`SocketAccessPolicy`]).

use std::collections::BTreeSet;

use anyhow::Result;
use quanta_index_ipc::{SharedSocketAccess, SocketAccessPolicy};

/// The three sockets the daemon binds, in bind order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketRole {
    Query,
    Control,
    Ingest,
}

impl SocketRole {
    pub const ALL: [Self; 3] = [Self::Query, Self::Control, Self::Ingest];

    /// The role's name as it appears in metric names and env knobs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Control => "control",
            Self::Ingest => "ingest",
        }
    }

    /// The env knob that sets this socket's access policy.
    #[must_use]
    pub const fn env_name(self) -> &'static str {
        match self {
            Self::Query => "QUANTA_INDEX_QUERY_SOCKET_ACCESS",
            Self::Control => "QUANTA_INDEX_CONTROL_SOCKET_ACCESS",
            Self::Ingest => "QUANTA_INDEX_INGEST_SOCKET_ACCESS",
        }
    }
}

/// One access policy per socket role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketAccessPolicies {
    query: SocketAccessPolicy,
    control: SocketAccessPolicy,
    ingest: SocketAccessPolicy,
}

impl SocketAccessPolicies {
    /// Every socket private: the daemon's default.
    pub const PRIVATE: Self = Self {
        query: SocketAccessPolicy::Private,
        control: SocketAccessPolicy::Private,
        ingest: SocketAccessPolicy::Private,
    };

    #[must_use]
    pub const fn new(
        query: SocketAccessPolicy,
        control: SocketAccessPolicy,
        ingest: SocketAccessPolicy,
    ) -> Self {
        Self {
            query,
            control,
            ingest,
        }
    }

    #[must_use]
    pub const fn for_role(&self, role: SocketRole) -> &SocketAccessPolicy {
        match role {
            SocketRole::Query => &self.query,
            SocketRole::Control => &self.control,
            SocketRole::Ingest => &self.ingest,
        }
    }
}

impl Default for SocketAccessPolicies {
    fn default() -> Self {
        Self::PRIVATE
    }
}

/// Where names in an access knob are resolved to ids.
///
/// A port so the knob grammar is provable without touching the system's
/// user and group databases; the daemon resolves through
/// [`SystemPrincipals`].
pub trait PrincipalResolver {
    /// The gid of the group called `name`, or `None` when no such group
    /// exists. An error is a lookup failure, not an absent group.
    fn gid_of_group(&self, name: &str) -> Result<Option<u32>>;
    /// The uid of the user called `name`, or `None` when no such user
    /// exists.
    fn uid_of_user(&self, name: &str) -> Result<Option<u32>>;
}

/// The system's user and group databases (`getgrnam_r`, `getpwnam_r`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemPrincipals;

impl PrincipalResolver for SystemPrincipals {
    fn gid_of_group(&self, name: &str) -> Result<Option<u32>> {
        let group = nix::unistd::Group::from_name(name)
            .map_err(|error| anyhow::anyhow!("looking up group `{name}` failed: {error}"))?;
        Ok(group.map(|group| group.gid.as_raw()))
    }

    fn uid_of_user(&self, name: &str) -> Result<Option<u32>> {
        let user = nix::unistd::User::from_name(name)
            .map_err(|error| anyhow::anyhow!("looking up user `{name}` failed: {error}"))?;
        Ok(user.map(|user| user.uid.as_raw()))
    }
}

/// Resolve the three socket access policies from an injected lookup and
/// resolver. Every knob is independent; an unset knob is `private`. The
/// config chain applies it on both entry points.
pub(crate) fn socket_access_policies_from_lookup(
    lookup: &dyn Fn(&str) -> Result<Option<String>>,
    principals: &dyn PrincipalResolver,
) -> Result<SocketAccessPolicies> {
    let mut policies = [
        SocketAccessPolicy::Private,
        SocketAccessPolicy::Private,
        SocketAccessPolicy::Private,
    ];
    for (slot, role) in policies.iter_mut().zip(SocketRole::ALL) {
        if let Some(raw) = lookup(role.env_name())? {
            *slot = parse_socket_access(role.env_name(), &raw, principals)?;
        }
    }
    let [query, control, ingest] = policies;
    Ok(SocketAccessPolicies::new(query, control, ingest))
}

/// Parse one knob's value under the module grammar, resolving names
/// through `principals`.
pub(crate) fn parse_socket_access(
    knob: &str,
    raw: &str,
    principals: &dyn PrincipalResolver,
) -> Result<SocketAccessPolicy> {
    let value = raw.trim();
    if value == "private" {
        return Ok(SocketAccessPolicy::Private);
    }
    let Some(items) = value.strip_prefix("shared:") else {
        return Err(anyhow::anyhow!(
            "{knob} must be `private` or `shared:group=<group>[,uid=<user>...]`, got `{raw}`"
        ));
    };
    let mut group: Option<u32> = None;
    let mut allowed_uids = BTreeSet::new();
    for item in items.split(',') {
        let Some((key, operand)) = item.split_once('=') else {
            return Err(anyhow::anyhow!(
                "{knob}: item `{item}` is not `group=<group>` or `uid=<user>`"
            ));
        };
        if operand.is_empty() || operand.chars().any(char::is_whitespace) {
            return Err(anyhow::anyhow!(
                "{knob}: item `{item}` has an empty or whitespace-bearing value"
            ));
        }
        match key {
            "group" => {
                if group.is_some() {
                    return Err(anyhow::anyhow!("{knob}: `group=` may be given once"));
                }
                group = Some(resolve_group(knob, operand, principals)?);
            }
            "uid" => {
                let uid = resolve_user(knob, operand, principals)?;
                if !allowed_uids.insert(uid) {
                    return Err(anyhow::anyhow!(
                        "{knob}: uid {uid} (`{operand}`) is listed more than once"
                    ));
                }
            }
            other => {
                return Err(anyhow::anyhow!(
                    "{knob}: unknown item `{other}=`; items are `group=` and `uid=`"
                ));
            }
        }
    }
    let access = SharedSocketAccess::new(group, allowed_uids);
    if !access.admits_others() {
        return Err(anyhow::anyhow!(
            "{knob}: `shared:` names no group and no uid, which admits nobody but the owner; say `private`"
        ));
    }
    Ok(SocketAccessPolicy::Shared(access))
}

fn resolve_group(knob: &str, operand: &str, principals: &dyn PrincipalResolver) -> Result<u32> {
    if let Ok(gid) = operand.parse::<u32>() {
        return Ok(gid);
    }
    principals
        .gid_of_group(operand)
        .map_err(|error| anyhow::anyhow!("{knob}: {error}"))?
        .ok_or_else(|| anyhow::anyhow!("{knob}: group `{operand}` does not exist on this host"))
}

fn resolve_user(knob: &str, operand: &str, principals: &dyn PrincipalResolver) -> Result<u32> {
    if let Ok(uid) = operand.parse::<u32>() {
        return Ok(uid);
    }
    principals
        .uid_of_user(operand)
        .map_err(|error| anyhow::anyhow!("{knob}: {error}"))?
        .ok_or_else(|| anyhow::anyhow!("{knob}: user `{operand}` does not exist on this host"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use anyhow::Result;
    use quanta_index_ipc::{SharedSocketAccess, SocketAccessPolicy};

    use super::{
        PrincipalResolver, SocketAccessPolicies, SocketRole, SystemPrincipals, parse_socket_access,
        socket_access_policies_from_lookup,
    };

    /// A scripted database: `devs` is gid 2000, `alice` is uid 1001,
    /// `flaky` fails to look up, everything else is absent.
    struct Scripted;

    impl PrincipalResolver for Scripted {
        fn gid_of_group(&self, name: &str) -> Result<Option<u32>> {
            match name {
                "devs" => Ok(Some(2000)),
                "flaky" => Err(anyhow::anyhow!("nss is down")),
                _ => Ok(None),
            }
        }

        fn uid_of_user(&self, name: &str) -> Result<Option<u32>> {
            match name {
                "alice" => Ok(Some(1001)),
                "flaky" => Err(anyhow::anyhow!("nss is down")),
                _ => Ok(None),
            }
        }
    }

    const KNOB: &str = "QUANTA_INDEX_QUERY_SOCKET_ACCESS";

    fn shared(group: Option<u32>, uids: &[u32]) -> SocketAccessPolicy {
        SocketAccessPolicy::Shared(SharedSocketAccess::new(
            group,
            uids.iter().copied().collect::<BTreeSet<u32>>(),
        ))
    }

    #[test]
    fn the_grammar_resolves_names_and_ids() {
        let cases: [(&str, SocketAccessPolicy); 6] = [
            ("private", SocketAccessPolicy::Private),
            ("  private  ", SocketAccessPolicy::Private),
            ("shared:group=devs", shared(Some(2000), &[])),
            ("shared:group=2001", shared(Some(2001), &[])),
            ("shared:uid=alice,uid=1002", shared(None, &[1001, 1002])),
            ("shared:uid=1002,group=devs,uid=alice", shared(Some(2000), &[1001, 1002])),
        ];
        for (raw, expected) in cases {
            let parsed = parse_socket_access(KNOB, raw, &Scripted).expect(raw);
            assert_eq!(parsed, expected, "{raw}");
        }
    }

    /// Every malformed spelling is refused with the knob named and the
    /// offending piece quoted; nothing is defaulted.
    #[test]
    fn malformed_and_unresolvable_values_are_refused() {
        let cases: [(&str, &str); 12] = [
            ("", "must be `private` or `shared:"),
            ("open", "must be `private` or `shared:"),
            ("shared", "must be `private` or `shared:"),
            ("shared:", "is not `group=<group>` or `uid=<user>`"),
            ("shared:group", "is not `group=<group>` or `uid=<user>`"),
            ("shared:group=", "empty or whitespace-bearing value"),
            ("shared:group=dev s", "empty or whitespace-bearing value"),
            ("shared:gid=devs", "unknown item `gid=`"),
            ("shared:group=devs,group=devs", "`group=` may be given once"),
            ("shared:uid=alice,uid=1001", "listed more than once"),
            ("shared:group=nope", "group `nope` does not exist"),
            ("shared:uid=nobody-here", "user `nobody-here` does not exist"),
        ];
        for (raw, expected) in cases {
            let error = parse_socket_access(KNOB, raw, &Scripted)
                .expect_err(raw)
                .to_string();
            assert!(error.contains(KNOB), "{raw}: {error}");
            assert!(error.contains(expected), "{raw}: {error}");
        }
        let lookup_failure = parse_socket_access(KNOB, "shared:group=flaky", &Scripted)
            .expect_err("a lookup failure is not an absent group")
            .to_string();
        assert!(lookup_failure.contains("nss is down"), "{lookup_failure}");
    }

    /// Unset knobs are private; each knob binds only its own socket.
    #[test]
    fn each_socket_is_private_unless_its_own_knob_says_otherwise() {
        let unset = socket_access_policies_from_lookup(&|_name| Ok(None), &Scripted)
            .expect("no knobs selects private everywhere");
        assert_eq!(unset, SocketAccessPolicies::PRIVATE);

        let query_only = socket_access_policies_from_lookup(
            &|name| {
                Ok((name == SocketRole::Query.env_name()).then(|| "shared:group=devs".to_string()))
            },
            &Scripted,
        )
        .expect("a shared query socket");
        assert_eq!(query_only.for_role(SocketRole::Query), &shared(Some(2000), &[]));
        assert_eq!(query_only.for_role(SocketRole::Control), &SocketAccessPolicy::Private);
        assert_eq!(query_only.for_role(SocketRole::Ingest), &SocketAccessPolicy::Private);

        let ingest_bad = socket_access_policies_from_lookup(
            &|name| Ok((name == SocketRole::Ingest.env_name()).then(|| "shared:uid=".to_string())),
            &Scripted,
        )
        .expect_err("a malformed ingest knob fails the whole config");
        assert!(
            ingest_bad
                .to_string()
                .contains(SocketRole::Ingest.env_name()),
            "{ingest_bad}"
        );
    }

    /// The daemon's own resolver refuses a group this host does not have,
    /// naming the knob and the group — the config fails before any socket
    /// is bound, since the config precedes the runtime.
    #[test]
    fn an_unknown_group_name_is_refused_through_the_system_resolver() {
        let error = socket_access_policies_from_lookup(
            &|name| {
                Ok((name == SocketRole::Query.env_name())
                    .then(|| "shared:group=no-such-group-xyz".to_string()))
            },
            &SystemPrincipals,
        )
        .expect_err("an unknown group name must refuse the config")
        .to_string();
        assert!(error.contains(SocketRole::Query.env_name()), "{error}");
        assert!(error.contains("group `no-such-group-xyz` does not exist"), "{error}");
    }

    /// The system resolver answers for this process's own primary group
    /// and user, and says `None` for a name that cannot exist.
    #[test]
    fn the_system_resolver_knows_this_process() -> Result<()> {
        let own_group = rustix::process::getegid().as_raw();
        let group = nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(own_group))?
            .ok_or_else(|| anyhow::anyhow!("this process's primary group has no entry"))?;
        let resolved = SystemPrincipals.gid_of_group(&group.name)?;
        if resolved != Some(own_group) {
            return Err(anyhow::anyhow!(
                "group `{}` resolved to {resolved:?}, expected {own_group}",
                group.name
            ));
        }
        let own_user = rustix::process::geteuid().as_raw();
        let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(own_user))?
            .ok_or_else(|| anyhow::anyhow!("this process's user has no entry"))?;
        let resolved = SystemPrincipals.uid_of_user(&user.name)?;
        if resolved != Some(own_user) {
            return Err(anyhow::anyhow!(
                "user `{}` resolved to {resolved:?}, expected {own_user}",
                user.name
            ));
        }
        if SystemPrincipals
            .gid_of_group("no-such-group-xyz-quanta")?
            .is_some()
        {
            return Err(anyhow::anyhow!("an impossible group name resolved"));
        }
        Ok(())
    }
}
