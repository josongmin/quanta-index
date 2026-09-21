use quanta_index_contract::{
    CandidateObjectDigestV1, QuarantineIncidentDigestV1, QuarantinePayloadDigestV1,
    StateRootUuidCommitmentV1,
};
use quanta_index_repomap::layout_v3::{LayoutV3AddressError, ObservedFileKindV1};
use quanta_index_repomap::{
    CandidateObjectAddressV1, ObservedFileMetadataV1, QuarantineIncidentAddressV1,
    QuarantinePayloadAddressV1, SecureMetadataPairV1, StateRootSecurityContextV1,
    StateRootSecurityVerificationErrorV1,
};

#[test]
fn typed_addresses_have_exact_fixed_fanout_and_disjoint_roots() {
    let candidate = CandidateObjectAddressV1::new(CandidateObjectDigestV1::from_bytes([0xab; 32]));
    let incident =
        QuarantineIncidentAddressV1::new(QuarantineIncidentDigestV1::from_bytes([0xab; 32]));
    let payload =
        QuarantinePayloadAddressV1::new(QuarantinePayloadDigestV1::from_bytes([0xab; 32]));
    assert_eq!(
        candidate.relative_path().to_string_lossy(),
        format!("objects/sha256/ab/ab/{}.cbor", "ab".repeat(30))
    );
    assert_eq!(
        incident.relative_path().to_string_lossy(),
        format!("quarantine/incidents/sha256/ab/ab/{}.cbor", "ab".repeat(30))
    );
    assert_eq!(
        payload.relative_path().to_string_lossy(),
        format!("quarantine/payloads/sha256/ab/ab/{}.bin", "ab".repeat(30))
    );
    let leaf = format!("{}.cbor", "ab".repeat(30));
    let parsed = CandidateObjectAddressV1::parse_components(&[
        b"objects",
        b"sha256",
        b"ab",
        b"ab",
        leaf.as_bytes(),
    ])
    .expect("roundtrip components parse");
    assert_eq!(parsed, candidate);
    let bad_leaf = format!("{}.cbor", "AB".repeat(30));
    assert_eq!(
        CandidateObjectAddressV1::parse_components(&[
            b"objects",
            b"sha256",
            b"AB",
            b"AB",
            bad_leaf.as_bytes()
        ]),
        Err(LayoutV3AddressError::NonLowercaseHex)
    );
}

fn metadata(kind: ObservedFileKindV1, link_count: u64) -> ObservedFileMetadataV1 {
    ObservedFileMetadataV1 {
        device: 1,
        inode: 2,
        uid: 501,
        mode: 0o600,
        link_count,
        kind,
    }
}

#[test]
fn security_primitive_enforces_path_and_lstat_fstat_precedence() {
    let context =
        StateRootSecurityContextV1::new(501, StateRootUuidCommitmentV1::for_uuid_bytes([7; 16]));
    for components in [
        vec![],
        vec![b"".as_slice()],
        vec![b".".as_slice()],
        vec![b"..".as_slice()],
        vec![b"a/b".as_slice()],
        vec![b"nul\0".as_slice()],
    ] {
        assert_eq!(
            StateRootSecurityContextV1::validate_relative_components(&components),
            Err(StateRootSecurityVerificationErrorV1::InvalidRelativePath)
        );
    }
    assert!(StateRootSecurityContextV1::validate_relative_components(&[b"%".as_slice()]).is_ok());
    let valid = metadata(ObservedFileKindV1::RegularFile, 1);
    assert_eq!(
        context.verify_opened_file(
            &[],
            SecureMetadataPairV1 {
                lstat: valid,
                fstat: Some(valid)
            }
        ),
        Ok(())
    );
    assert_eq!(
        context.verify_opened_file(
            &[],
            SecureMetadataPairV1 {
                lstat: metadata(ObservedFileKindV1::Symlink, 2),
                fstat: None
            }
        ),
        Err(StateRootSecurityVerificationErrorV1::SymlinkEncountered)
    );
    assert_eq!(
        context.verify_opened_file(
            &[],
            SecureMetadataPairV1 {
                lstat: valid,
                fstat: None
            }
        ),
        Err(StateRootSecurityVerificationErrorV1::SecureOpenUnavailable)
    );
    assert_eq!(
        context.verify_opened_file(
            &[],
            SecureMetadataPairV1 {
                lstat: metadata(ObservedFileKindV1::RegularFile, 2),
                fstat: Some(valid)
            }
        ),
        Err(StateRootSecurityVerificationErrorV1::HardlinkEncountered)
    );
    let mut swapped = valid;
    swapped.inode = 3;
    assert_eq!(
        context.verify_opened_file(
            &[],
            SecureMetadataPairV1 {
                lstat: valid,
                fstat: Some(swapped)
            }
        ),
        Err(StateRootSecurityVerificationErrorV1::MetadataMismatch)
    );
    let mut directory = valid;
    directory.kind = ObservedFileKindV1::Directory;
    directory.mode = 0o700;
    let opened_directory = SecureMetadataPairV1 {
        lstat: directory,
        fstat: Some(directory),
    };
    assert_eq!(
        context.verify_opened_file(
            &[opened_directory],
            SecureMetadataPairV1 {
                lstat: valid,
                fstat: Some(valid),
            }
        ),
        Ok(())
    );
    directory.mode = 0o755;
    assert_eq!(
        context.verify_opened_file(
            &[SecureMetadataPairV1 {
                lstat: directory,
                fstat: Some(directory),
            }],
            SecureMetadataPairV1 {
                lstat: valid,
                fstat: Some(valid),
            }
        ),
        Err(StateRootSecurityVerificationErrorV1::MetadataMismatch)
    );
}

#[test]
fn security_metadata_uid_mode_type_device_and_link_matrix() {
    let context =
        StateRootSecurityContextV1::new(501, StateRootUuidCommitmentV1::for_uuid_bytes([7; 16]));
    let file = metadata(ObservedFileKindV1::RegularFile, 1);
    let mut directory = file;
    directory.kind = ObservedFileKindV1::Directory;
    directory.mode = 0o700;
    let pair = |lstat, fstat| SecureMetadataPairV1 {
        lstat,
        fstat: Some(fstat),
    };
    let reject_leaf = |lstat, fstat, expected| {
        assert_eq!(context.verify_opened_file(&[], pair(lstat, fstat)), Err(expected));
    };
    let reject_directory = |lstat, fstat, expected| {
        assert_eq!(
            context.verify_opened_file(&[pair(lstat, fstat)], pair(file, file)),
            Err(expected)
        );
    };
    let mismatch = StateRootSecurityVerificationErrorV1::MetadataMismatch;
    for uid in [500, 502] {
        let mut bad = file;
        bad.uid = uid;
        reject_leaf(bad, file, mismatch);
        reject_leaf(file, bad, mismatch);
        let mut bad_dir = directory;
        bad_dir.uid = uid;
        reject_directory(bad_dir, directory, mismatch);
        reject_directory(directory, bad_dir, mismatch);
    }
    for mode in [0o400, 0o644, 0o700] {
        let mut bad = file;
        bad.mode = mode;
        reject_leaf(bad, file, mismatch);
        reject_leaf(file, bad, mismatch);
    }
    for mode in [0o600, 0o755, 0o777] {
        let mut bad = directory;
        bad.mode = mode;
        reject_directory(bad, directory, mismatch);
        reject_directory(directory, bad, mismatch);
    }
    for kind in [ObservedFileKindV1::Directory, ObservedFileKindV1::Other] {
        let mut bad = file;
        bad.kind = kind;
        reject_leaf(bad, file, mismatch);
        reject_leaf(file, bad, mismatch);
    }
    for kind in [ObservedFileKindV1::RegularFile, ObservedFileKindV1::Other] {
        let mut bad = directory;
        bad.kind = kind;
        reject_directory(bad, directory, mismatch);
        reject_directory(directory, bad, mismatch);
    }
    let mut bad = file;
    bad.device = 3;
    reject_leaf(bad, file, mismatch);
    reject_leaf(file, bad, mismatch);
    let mut bad_dir = directory;
    bad_dir.device = 3;
    reject_directory(bad_dir, directory, mismatch);
    reject_directory(directory, bad_dir, mismatch);
    let hardlink = StateRootSecurityVerificationErrorV1::HardlinkEncountered;
    let mut linked = file;
    linked.link_count = 2;
    reject_leaf(linked, file, hardlink);
    reject_leaf(file, linked, hardlink);
    let mut symlink = file;
    symlink.kind = ObservedFileKindV1::Symlink;
    reject_leaf(symlink, linked, StateRootSecurityVerificationErrorV1::SymlinkEncountered);
    reject_leaf(linked, symlink, StateRootSecurityVerificationErrorV1::SymlinkEncountered);
    let mut directory_symlink = directory;
    directory_symlink.kind = ObservedFileKindV1::Symlink;
    reject_directory(
        directory_symlink,
        directory,
        StateRootSecurityVerificationErrorV1::SymlinkEncountered,
    );
    reject_directory(
        directory,
        directory_symlink,
        StateRootSecurityVerificationErrorV1::SymlinkEncountered,
    );
}

#[test]
fn security_reason_priority_is_global_across_traversed_components() {
    let context =
        StateRootSecurityContextV1::new(501, StateRootUuidCommitmentV1::for_uuid_bytes([7; 16]));
    let file = metadata(ObservedFileKindV1::RegularFile, 1);
    let mut directory = file;
    directory.kind = ObservedFileKindV1::Directory;
    directory.mode = 0o700;
    let mut insecure_directory = directory;
    insecure_directory.mode = 0o755;
    let mut symlink = file;
    symlink.kind = ObservedFileKindV1::Symlink;
    let mut hardlink = file;
    hardlink.link_count = 2;
    let pair = |lstat, fstat| SecureMetadataPairV1 { lstat, fstat };

    assert_eq!(
        context.verify_opened_file(
            &[pair(insecure_directory, Some(insecure_directory))],
            pair(symlink, None),
        ),
        Err(StateRootSecurityVerificationErrorV1::SymlinkEncountered)
    );
    assert_eq!(
        context.verify_opened_file(&[pair(directory, None)], pair(symlink, Some(symlink)),),
        Err(StateRootSecurityVerificationErrorV1::SymlinkEncountered)
    );
    assert_eq!(
        context.verify_opened_file(
            &[pair(insecure_directory, Some(insecure_directory))],
            pair(file, None),
        ),
        Err(StateRootSecurityVerificationErrorV1::SecureOpenUnavailable)
    );
    assert_eq!(
        context.verify_opened_file(
            &[pair(insecure_directory, Some(insecure_directory))],
            pair(hardlink, Some(hardlink)),
        ),
        Err(StateRootSecurityVerificationErrorV1::HardlinkEncountered)
    );
}
