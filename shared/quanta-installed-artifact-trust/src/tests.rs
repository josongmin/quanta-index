use super::*;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::Digest as _;
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::rc::Rc;

struct FixtureV1 {
    _root_v1: tempfile::TempDir,
    trust_v1: PathBuf,
    anchor_v1: PathBuf,
    server_v1: PathBuf,
    owner_uid_v1: u32,
}

fn write_v1(path_v1: &Path, bytes_v1: &[u8], mode_v1: u32) {
    fs::create_dir_all(path_v1.parent().expect("parent")).expect("mkdir");
    fs::write(path_v1, bytes_v1).expect("write");
    fs::set_permissions(path_v1, fs::Permissions::from_mode(mode_v1)).expect("mode");
}

impl FixtureV1 {
    fn new_v1() -> Self {
        let root_v1 = tempfile::tempdir().expect("tempdir");
        let anchor_v1 = root_v1.path().canonicalize().expect("canonical root");
        let owner_uid_v1 = anchor_v1.metadata().expect("metadata").uid();
        let trust_v1 = anchor_v1.join("trust");
        let install_v1 = anchor_v1.join("install");
        fs::create_dir_all(&trust_v1).expect("trust dir");
        fs::create_dir_all(install_v1.join("records")).expect("records dir");
        write_v1(&trust_v1.join("rotation.lock"), b"", 0o444);
        let signer_v1 = SigningKey::from_bytes(&[7u8; 32]);
        write_v1(
            &trust_v1.join("deployment-public-key.bin"),
            &signer_v1.verifying_key().to_bytes(), 0o444,
        );
        let server_bytes_v1 = b"server-image";
        let server_sha_v1 = sha256_v1(server_bytes_v1);
        let manifest_v1 = serde_json::json!({
            "schema_version": "quanta-release-bundle-v4",
            "artifacts": [{
                "role": "server",
                "relative_path": "libexec/quanta/quanta-cli-server",
                "sha256": server_sha_v1,
                "size_bytes": server_bytes_v1.len(),
            }],
        });
        let manifest_bytes_v1 = canonical_value_v1(&manifest_v1).expect("manifest");
        let manifest_sha_v1 = sha256_v1(&manifest_bytes_v1);
        let release_v1 = install_v1.join("releases").join(&manifest_sha_v1);
        write_v1(&release_v1.join("share/quanta/release-manifest.json"), &manifest_bytes_v1, 0o444);
        let server_v1 = release_v1.join("libexec/quanta/quanta-cli-server");
        write_v1(&server_v1, server_bytes_v1, 0o555);
        for (relative_v1, bytes_v1) in [
            ("bin/quanta", b"cli-image".as_slice()),
            ("libexec/quanta/quanta-sql-rulepack-worker", b"sql-image".as_slice()),
            ("libexec/quanta/quanta_native_worker_v3", b"parser-image".as_slice()),
        ] {
            write_v1(&release_v1.join(relative_v1), bytes_v1, 0o555);
        }
        let active_record_v1 = serde_json::json!({"payload": {
            "generation": 1,
            "active_manifest_sha256": manifest_sha_v1,
            "install_root": install_v1.to_str().expect("utf8"),
        }});
        let mut active_record_bytes_v1 = canonical_value_v1(&active_record_v1).expect("active record");
        active_record_bytes_v1.push(b'\n');
        let active_record_sha_v1 = sha256_v1(&active_record_bytes_v1);
        write_v1(&install_v1.join("records/active.json"), &active_record_bytes_v1, 0o444);
        let payload_v1 = serde_json::json!({
            "schema_version": ACTIVE_SCHEMA_V1,
            "install_root": install_v1.to_str().expect("utf8"),
            "generation": 1,
            "ag5_active_record_sha256": active_record_sha_v1,
            "release_manifest_sha256": manifest_sha_v1,
            "cli_sha256": sha256_v1(b"cli-image"),
            "server_sha256": server_sha_v1,
            "sql_rulepack_worker_sha256": sha256_v1(b"sql-image"),
            "parser_native_worker_sha256": sha256_v1(b"parser-image"),
        });
        let mut signed_v1 = ACTIVE_DOMAIN_V1.to_vec();
        signed_v1.extend(canonical_value_v1(&payload_v1).expect("signed payload"));
        let signature_v1 = signer_v1.sign(&signed_v1);
        let signature_hex_v1 = signature_v1.to_bytes().iter()
            .map(|byte_v1| format!("{byte_v1:02x}"))
            .collect::<String>();
        let envelope_v1 = serde_json::json!({"payload": payload_v1, "signature_hex": signature_hex_v1});
        let mut envelope_bytes_v1 = canonical_value_v1(&envelope_v1).expect("envelope");
        envelope_bytes_v1.push(b'\n');
        write_v1(&trust_v1.join("active-generation.json"), &envelope_bytes_v1, 0o444);
        Self { _root_v1: root_v1, trust_v1, anchor_v1, server_v1, owner_uid_v1 }
    }

    fn verify_v1<A: InstalledArtifactTrustAdmissionV1, R>(
        &self, peer_v1: &File, admission_v1: &mut A,
        use_verified_v1: impl FnOnce(&VerifiedInstalledServerPeerV1<A::MemoryLeaseV1>, &mut A) -> R,
    ) -> Result<R, InstalledArtifactPeerErrorV1<A::RefusalV1>> {
        with_verified_installed_server_peer_fixture_v1(
            peer_v1, admission_v1, &self.trust_v1, &self.anchor_v1,
            self.owner_uid_v1, use_verified_v1,
        )
    }

    fn resign_active_v2(&self, recovery_v2: Value, domain_v2: &[u8]) {
        self.resign_active_with_grant_v1(
            ACTIVE_SCHEMA_V2, "native_recovery", recovery_v2, domain_v2,
        );
    }

    fn resign_active_v3(&self, publication_v3: Value, domain_v3: &[u8]) {
        self.resign_active_with_grant_v1(
            ACTIVE_SCHEMA_V3, "native_facts_publication", publication_v3, domain_v3,
        );
    }

    fn resign_active_with_grant_v1(
        &self, schema_v1: &str, grant_name_v1: &str, grant_v1: Value, domain_v1: &[u8],
    ) {
        let active_v1 = self.trust_v1.join("active-generation.json");
        let mut envelope_v1: Value =
            serde_json::from_slice(&fs::read(&active_v1).expect("active envelope"))
                .expect("active JSON");
        envelope_v1["payload"]["schema_version"] = Value::String(schema_v1.to_owned());
        envelope_v1["payload"][grant_name_v1] = grant_v1;
        let mut signed_v1 = domain_v1.to_vec();
        signed_v1.extend(canonical_value_v1(&envelope_v1["payload"]).expect("signed payload"));
        let signer_v1 = SigningKey::from_bytes(&[7u8; 32]);
        envelope_v1["signature_hex"] = Value::String(
            signer_v1.sign(&signed_v1).to_bytes().iter()
                .map(|byte_v1| format!("{byte_v1:02x}"))
                .collect(),
        );
        let mut bytes_v1 = canonical_value_v1(&envelope_v1).expect("signed envelope");
        bytes_v1.push(b'\n');
        fs::set_permissions(&active_v1, fs::Permissions::from_mode(0o644)).expect("mutable active");
        fs::write(&active_v1, bytes_v1).expect("write signed active");
        fs::set_permissions(&active_v1, fs::Permissions::from_mode(0o444)).expect("seal active");
    }
}

fn native_recovery_fixture_v2() -> Value {
    serde_json::json!({
        "analysis_family": "taint_rows_ifds",
        "repository_root": "/srv/semantica/repo",
        "q_image_sha256": sha256_v1(b"Q image"),
        "q_config_sha256": sha256_v1(b"Q config"),
    })
}

fn native_facts_publication_fixture_v3() -> Value {
    serde_json::json!({
        "repository_root": "/srv/semantica/facts-repo",
        "q_image_sha256": sha256_v1(b"Facts Q image"),
        "q_config_sha256": sha256_v1(b"Facts Q config"),
    })
}

#[test]
fn signed_v3_native_facts_publication_issues_only_exact_typed_token() {
    let fixture_v3 = FixtureV1::new_v1();
    fixture_v3.resign_active_v3(native_facts_publication_fixture_v3(), ACTIVE_DOMAIN_V3);
    let installed_v3 = verify_full_installed_artifacts_fixture_v1(
        &fixture_v3.trust_v1, &fixture_v3.anchor_v1, fixture_v3.owner_uid_v1,
    ).expect("verified V3 installation");
    assert!(installed_v3.native_recovery_v2.is_none());
    let facts_v3 = installed_v3.native_facts_publication_v3.expect("V3 Facts authority");
    assert_eq!(facts_v3.repository_root_v3(), "/srv/semantica/facts-repo");
    let q_image_v3: [u8; 32] = Sha256::digest(b"Facts Q image").into();
    let q_config_v3: [u8; 32] = Sha256::digest(b"Facts Q config").into();
    assert_eq!(facts_v3.q_image_sha256_v3(), &q_image_v3);
    assert_eq!(facts_v3.q_config_sha256_v3(), &q_config_v3);
}

#[test]
fn signed_v3_native_facts_publication_refuses_v2_domain_and_dual_grants() {
    let fixture_v3 = FixtureV1::new_v1();
    fixture_v3.resign_active_v3(native_facts_publication_fixture_v3(), ACTIVE_DOMAIN_V2);
    assert!(matches!(verify_full_installed_artifacts_fixture_v1(
        &fixture_v3.trust_v1, &fixture_v3.anchor_v1, fixture_v3.owner_uid_v1,
    ), Err(InstalledTrustErrorV1::InvalidSignature)));

    let fixture_v3 = FixtureV1::new_v1();
    let mut both_v3 = native_facts_publication_fixture_v3();
    both_v3["analysis_family"] = Value::String("taint_rows_ifds".to_owned());
    fixture_v3.resign_active_v3(both_v3, ACTIVE_DOMAIN_V3);
    assert!(matches!(verify_full_installed_artifacts_fixture_v1(
        &fixture_v3.trust_v1, &fixture_v3.anchor_v1, fixture_v3.owner_uid_v1,
    ), Err(InstalledTrustErrorV1::InvalidActiveGeneration)));

    let fixture_v3 = FixtureV1::new_v1();
    fixture_v3.resign_active_v2(native_recovery_fixture_v2(), ACTIVE_DOMAIN_V2);
    fixture_v3.resign_active_v3(native_facts_publication_fixture_v3(), ACTIVE_DOMAIN_V3);
    assert!(matches!(verify_full_installed_artifacts_fixture_v1(
        &fixture_v3.trust_v1, &fixture_v3.anchor_v1, fixture_v3.owner_uid_v1,
    ), Err(InstalledTrustErrorV1::InvalidActiveGeneration)));
}

#[test]
fn signed_v3_native_facts_publication_refuses_invalid_root_digest_and_extra_field() {
    for (field_v3, value_v3) in [
        ("repository_root", Value::String("/srv/../foreign".to_owned())),
        ("q_image_sha256", Value::String("0".repeat(64))),
        ("q_config_sha256", Value::String("A".repeat(64))),
        ("foreign", Value::String("unexpected".to_owned())),
    ] {
        let fixture_v3 = FixtureV1::new_v1();
        let mut publication_v3 = native_facts_publication_fixture_v3();
        publication_v3[field_v3] = value_v3;
        fixture_v3.resign_active_v3(publication_v3, ACTIVE_DOMAIN_V3);
        assert!(matches!(verify_full_installed_artifacts_fixture_v1(
            &fixture_v3.trust_v1, &fixture_v3.anchor_v1, fixture_v3.owner_uid_v1,
        ), Err(InstalledTrustErrorV1::InvalidActiveGeneration)), "accepted {field_v3}");
    }
}

#[test]
fn signed_v2_native_recovery_issues_exact_typed_token() {
    let fixture_v2 = FixtureV1::new_v1();
    fixture_v2.resign_active_v2(native_recovery_fixture_v2(), ACTIVE_DOMAIN_V2);
    let installed_v2 = verify_full_installed_artifacts_fixture_v1(
        &fixture_v2.trust_v1, &fixture_v2.anchor_v1, fixture_v2.owner_uid_v1,
    ).expect("verified V2 installation");
    let recovery_v2 = installed_v2.native_recovery_v2.expect("V2 recovery authority");
    assert_eq!(recovery_v2.repository_root_v2(), "/srv/semantica/repo");
    let q_image_v2: [u8; 32] = Sha256::digest(b"Q image").into();
    let q_config_v2: [u8; 32] = Sha256::digest(b"Q config").into();
    assert_eq!(recovery_v2.q_image_sha256_v2(), &q_image_v2);
    assert_eq!(recovery_v2.q_config_sha256_v2(), &q_config_v2);
}

#[test]
fn signed_v2_native_recovery_rejects_v1_domain_and_invalid_fields() {
    let fixture_v2 = FixtureV1::new_v1();
    fixture_v2.resign_active_v2(native_recovery_fixture_v2(), ACTIVE_DOMAIN_V1);
    assert!(matches!(verify_full_installed_artifacts_fixture_v1(
        &fixture_v2.trust_v1, &fixture_v2.anchor_v1, fixture_v2.owner_uid_v1,
    ), Err(InstalledTrustErrorV1::InvalidSignature)));

    for bad_root_v2 in ["/", "/srv//repo", "/srv/../repo", "/srv/repo/", "/srv/./repo", "relative", "/srv/repo\n"] {
        let fixture_v2 = FixtureV1::new_v1();
        let mut recovery_v2 = native_recovery_fixture_v2();
        recovery_v2["repository_root"] = Value::String(bad_root_v2.to_owned());
        fixture_v2.resign_active_v2(recovery_v2, ACTIVE_DOMAIN_V2);
        assert!(matches!(verify_full_installed_artifacts_fixture_v1(
            &fixture_v2.trust_v1, &fixture_v2.anchor_v1, fixture_v2.owner_uid_v1,
        ), Err(InstalledTrustErrorV1::InvalidActiveGeneration)), "accepted {bad_root_v2:?}");
    }
}

struct LeaseV1(Rc<Cell<usize>>);
impl Drop for LeaseV1 {
    fn drop(&mut self) { self.0.set(self.0.get() - 1); }
}

struct AdmissionV1 {
    live_v1: Rc<Cell<usize>>,
    reads_v1: u64,
    hashes_v1: u64,
    refuse_v1: bool,
}
impl AdmissionV1 {
    fn new_v1() -> Self {
        Self { live_v1: Rc::new(Cell::new(0)), reads_v1: 0, hashes_v1: 0, refuse_v1: false }
    }
}
impl InstalledArtifactTrustAdmissionV1 for AdmissionV1 {
    type RefusalV1 = &'static str;
    type MemoryLeaseV1 = LeaseV1;
    fn checkpoint_v1(&mut self) -> Result<(), Self::RefusalV1> {
        if self.refuse_v1 { Err("cancelled") } else { Ok(()) }
    }
    fn charge_io_v1(&mut self, _operations_v1: u64) -> Result<(), Self::RefusalV1> { Ok(()) }
    fn charge_read_v1(&mut self, bytes_v1: u64) -> Result<(), Self::RefusalV1> {
        self.reads_v1 += bytes_v1; Ok(())
    }
    fn charge_hash_v1(&mut self, bytes_v1: u64) -> Result<(), Self::RefusalV1> {
        self.hashes_v1 += bytes_v1; Ok(())
    }
    fn reserve_memory_v1(&mut self, _bytes_v1: usize) -> Result<Self::MemoryLeaseV1, Self::RefusalV1> {
        self.live_v1.set(self.live_v1.get() + 1);
        Ok(LeaseV1(self.live_v1.clone()))
    }
}

#[test]
fn signed_server_peer_retains_files_rotation_and_memory_until_callback_v1() {
    let fixture_v1 = FixtureV1::new_v1();
    let peer_v1 = File::open(&fixture_v1.server_v1).expect("peer");
    let mut admission_v1 = AdmissionV1::new_v1();
    let live_v1 = admission_v1.live_v1.clone();
    let expected_sha_v1: [u8; 32] = Sha256::digest(b"server-image").into();
    fixture_v1.verify_v1(&peer_v1, &mut admission_v1, |token_v1, admission_v1| {
        assert!(live_v1.get() > 0);
        assert_eq!(token_v1.generation_v1(), 1);
        assert_eq!(token_v1.server_image_sha256_bytes_v1(), &expected_sha_v1);
        assert_eq!(token_v1.server_image_v1().metadata().expect("server metadata").len(), 12);
        assert!(token_v1.manifest_image_v1().metadata().expect("manifest metadata").len() > 0);
        assert!(admission_v1.reads_v1 > 0 && admission_v1.hashes_v1 > 0);
        let competing_v1 = File::open(fixture_v1.trust_v1.join("rotation.lock")).expect("lock file");
        assert!(fs2::FileExt::try_lock_exclusive(&competing_v1).is_err());
    }).expect("verified server peer");
    assert_eq!(live_v1.get(), 0);
}

#[test]
fn foreign_peer_and_cancelled_admission_fail_closed_v1() {
    let fixture_v1 = FixtureV1::new_v1();
    let foreign_v1 = fixture_v1.trust_v1.join("foreign");
    write_v1(&foreign_v1, b"foreign-image", 0o555);
    let foreign_v1 = File::open(foreign_v1).expect("foreign peer");
    let mut admission_v1 = AdmissionV1::new_v1();
    assert!(matches!(fixture_v1.verify_v1(&foreign_v1, &mut admission_v1, |_, _| ()),
        Err(InstalledArtifactPeerErrorV1::Trust(InstalledTrustErrorV1::ForeignImage))));
    admission_v1.refuse_v1 = true;
    let peer_v1 = File::open(&fixture_v1.server_v1).expect("peer");
    assert!(matches!(fixture_v1.verify_v1(&peer_v1, &mut admission_v1, |_, _| ()),
        Err(InstalledArtifactPeerErrorV1::Admission("cancelled"))));
    assert_eq!(admission_v1.live_v1.get(), 0);
}

#[test]
fn signed_generation_and_server_mutation_are_refused_v1() {
    let fixture_v1 = FixtureV1::new_v1();
    let peer_v1 = File::open(&fixture_v1.server_v1).expect("peer");
    let mut admission_v1 = AdmissionV1::new_v1();
    fs::set_permissions(&fixture_v1.server_v1, fs::Permissions::from_mode(0o644))
        .expect("make mutable for adversarial fixture");
    fs::write(&fixture_v1.server_v1, b"changed-image").expect("mutate server");
    fs::set_permissions(&fixture_v1.server_v1, fs::Permissions::from_mode(0o555))
        .expect("restore executable mode");
    assert!(matches!(fixture_v1.verify_v1(&peer_v1, &mut admission_v1, |_, _| ()),
        Err(InstalledArtifactPeerErrorV1::Trust(InstalledTrustErrorV1::ChangedImage))));

    let fixture_v1 = FixtureV1::new_v1();
    let peer_v1 = File::open(&fixture_v1.server_v1).expect("peer");
    let active_v1 = fixture_v1.trust_v1.join("active-generation.json");
    fs::set_permissions(&active_v1, fs::Permissions::from_mode(0o644)).expect("mutable active");
    let mut envelope_v1: Value = serde_json::from_slice(&fs::read(&active_v1).expect("read active"))
        .expect("envelope JSON");
    let signature_v1 = envelope_v1["signature_hex"].as_str().expect("signature").to_owned();
    let first_v1 = if signature_v1.starts_with('0') { "1" } else { "0" };
    envelope_v1["signature_hex"] = Value::String(format!("{first_v1}{}", &signature_v1[1..]));
    let mut raw_v1 = canonical_value_v1(&envelope_v1).expect("mutated envelope");
    raw_v1.push(b'\n');
    fs::write(&active_v1, raw_v1).expect("mutate active signature");
    fs::set_permissions(&active_v1, fs::Permissions::from_mode(0o444)).expect("restore mode");
    assert!(matches!(fixture_v1.verify_v1(&peer_v1, &mut admission_v1, |_, _| ()),
        Err(InstalledArtifactPeerErrorV1::Trust(InstalledTrustErrorV1::InvalidSignature))));
}
