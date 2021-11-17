//! # Known Answer Tests for the key schedule
//!
//! See https://github.com/mlswg/mls-implementations/blob/master/test-vectors.md
//! for more description on the test vectors.
//!
//! If values are not present, they are encoded as empty strings.

use std::convert::TryFrom;

use crate::{
    ciphersuite::{Ciphersuite, CiphersuiteName, Secret},
    config::{Config, ProtocolVersion},
    group::{GroupContext, GroupEpoch, GroupId},
    prelude::{BranchPsk, Psk, PskType::Branch},
    schedule::{EpochSecrets, InitSecret, JoinerSecret, KeySchedule, WelcomeSecret},
    test_utils::{bytes_to_hex, hex_to_bytes},
};

#[cfg(test)]
use crate::test_utils::{read, write};

use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::{random::OpenMlsRand, types::HpkeKeyPair, OpenMlsCryptoProvider};
use rand::{rngs::OsRng, RngCore};
use serde::{self, Deserialize, Serialize};

use super::{errors::KsTestVectorError, PskSecret};
use super::{CommitSecret, PreSharedKeyId};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct PskValue {
    psk_id: String, /* hex encoded PreSharedKeyID */
    psk: String,    /* hex-encoded binary data */
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct Epoch {
    // Chosen by the generator
    tree_hash: String,
    commit_secret: String,
    // XXX: PSK is not supported in OpenMLS yet #141
    psks: Vec<PskValue>,
    confirmed_transcript_hash: String,

    // Computed values
    group_context: String,
    joiner_secret: String,
    welcome_secret: String,
    init_secret: String,
    sender_data_secret: String,
    encryption_secret: String,
    exporter_secret: String,
    authentication_secret: String,
    external_secret: String,
    confirmation_key: String,
    membership_key: String,
    resumption_secret: String,

    external_pub: String, // TLS serialized HpkePublicKey
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct KeyScheduleTestVector {
    pub cipher_suite: u16,
    group_id: String,
    initial_init_secret: String,
    epochs: Vec<Epoch>,
}

fn generate(
    ciphersuite: &'static Ciphersuite,
    init_secret: &InitSecret,
    group_id: &[u8],
    epoch: u64,
) -> (
    Vec<u8>,
    CommitSecret,
    JoinerSecret,
    Vec<(PreSharedKeyId, Secret)>,
    WelcomeSecret,
    EpochSecrets,
    Vec<u8>,
    GroupContext,
    HpkeKeyPair,
) {
    let crypto = OpenMlsRustCrypto::default();
    let tree_hash = crypto.rand().random_vec(ciphersuite.hash_length()).unwrap();
    let commit_secret = CommitSecret::random(ciphersuite, &crypto);

    // Build the PSK secret.
    let mut psk_ids = Vec::new();
    let mut psks = Vec::new();
    let mut psks_out = Vec::new();
    for _ in 0..(OsRng.next_u32() % 0x10) {
        let psk_id =
        // XXX: Test all different PSK types.
        PreSharedKeyId::new(
            Branch,
            Psk::Branch(BranchPsk {
                psk_group_id: GroupId::random(&crypto),
                psk_epoch: GroupEpoch(epoch),
            }),
            crypto.rand().random_vec(13).unwrap(),
        );
        let psk = PskSecret::random(ciphersuite, &crypto);
        psk_ids.push(psk_id.clone());
        psks.push(psk.secret().clone());
        psks_out.push((psk_id, psk.secret().clone()));
    }
    let psk_secret = PskSecret::new(ciphersuite, &crypto, &psk_ids, &psks).unwrap();

    let joiner_secret = JoinerSecret::new(&crypto, &commit_secret, init_secret);
    let mut key_schedule = KeySchedule::init(
        ciphersuite,
        &crypto,
        joiner_secret.clone(),
        Some(psk_secret.clone()),
    );
    let welcome_secret = key_schedule.welcome(&crypto).unwrap();

    let confirmed_transcript_hash = crypto.rand().random_vec(ciphersuite.hash_length()).unwrap();

    let group_context = GroupContext::new(
        GroupId::from_slice(group_id),
        GroupEpoch(epoch),
        tree_hash.to_vec(),
        confirmed_transcript_hash.clone(),
        &[], // Extensions
    )
    .unwrap();

    key_schedule.add_context(&crypto, &group_context).unwrap();
    let epoch_secrets = key_schedule.epoch_secrets(&crypto, true).unwrap();

    // Calculate external HPKE key pair
    let external_key_pair = epoch_secrets
        .external_secret()
        .derive_external_keypair(crypto.crypto(), ciphersuite);

    (
        confirmed_transcript_hash,
        commit_secret,
        joiner_secret,
        psks_out,
        welcome_secret,
        epoch_secrets,
        tree_hash,
        group_context,
        external_key_pair,
    )
}

#[cfg(any(feature = "test-utils", test))]
pub fn generate_test_vector(
    n_epochs: u64,
    ciphersuite: &'static Ciphersuite,
) -> KeyScheduleTestVector {
    use tls_codec::Serialize;

    use crate::ciphersuite::HpkePublicKey;
    let crypto = OpenMlsRustCrypto::default();

    // Set up setting.
    let mut init_secret = InitSecret::random(ciphersuite, &crypto, ProtocolVersion::default());
    let initial_init_secret = init_secret.clone();
    let group_id = crypto.rand().random_vec(16).unwrap();

    let mut epochs = Vec::new();

    // Generate info for all epochs
    for epoch in 0..n_epochs {
        println!("Generating epoch: {:?}", epoch);
        let (
            confirmed_transcript_hash,
            commit_secret,
            joiner_secret,
            psks,
            welcome_secret,
            epoch_secrets,
            tree_hash,
            group_context,
            external_key_pair,
        ) = generate(ciphersuite, &init_secret, &group_id, epoch);

        let psks = psks
            .iter()
            .map(|(psk_id, psk)| PskValue {
                psk_id: bytes_to_hex(&psk_id.tls_serialize_detached().unwrap()),
                psk: bytes_to_hex(psk.as_slice()),
            })
            .collect::<Vec<_>>();

        let epoch_info = Epoch {
            tree_hash: bytes_to_hex(&tree_hash),
            commit_secret: bytes_to_hex(commit_secret.as_slice()),
            psks,
            confirmed_transcript_hash: bytes_to_hex(&confirmed_transcript_hash),
            group_context: bytes_to_hex(&group_context.tls_serialize_detached().unwrap()),
            joiner_secret: bytes_to_hex(joiner_secret.as_slice()),
            welcome_secret: bytes_to_hex(welcome_secret.as_slice()),
            init_secret: bytes_to_hex(epoch_secrets.init_secret().unwrap().as_slice()),
            sender_data_secret: bytes_to_hex(epoch_secrets.sender_data_secret().as_slice()),
            encryption_secret: bytes_to_hex(epoch_secrets.encryption_secret().as_slice()),
            exporter_secret: bytes_to_hex(epoch_secrets.exporter_secret().as_slice()),
            authentication_secret: bytes_to_hex(epoch_secrets.authentication_secret().as_slice()),
            external_secret: bytes_to_hex(epoch_secrets.external_secret().as_slice()),
            confirmation_key: bytes_to_hex(epoch_secrets.confirmation_key().as_slice()),
            membership_key: bytes_to_hex(epoch_secrets.membership_key().as_slice()),
            resumption_secret: bytes_to_hex(epoch_secrets.resumption_secret().as_slice()),
            external_pub: bytes_to_hex(
                &HpkePublicKey::from(external_key_pair.public)
                    .tls_serialize_detached()
                    .unwrap(),
            ),
        };
        epochs.push(epoch_info);
        init_secret = epoch_secrets.init_secret().unwrap().clone();
    }

    KeyScheduleTestVector {
        cipher_suite: ciphersuite.name() as u16,
        group_id: bytes_to_hex(&group_id),
        initial_init_secret: bytes_to_hex(initial_init_secret.as_slice()),
        epochs,
    }
}

#[test]
fn write_test_vectors() {
    const NUM_EPOCHS: u64 = 200;
    let mut tests = Vec::new();
    for ciphersuite in Config::supported_ciphersuites() {
        tests.push(generate_test_vector(NUM_EPOCHS, ciphersuite));
    }
    write("test_vectors/kat_key_schedule_openmls-new.json", &tests);
}

#[test]
fn read_test_vectors() {
    let tests: Vec<KeyScheduleTestVector> = read("test_vectors/kat_key_schedule_openmls.json");
    for test_vector in tests {
        match run_test_vector(test_vector) {
            Ok(_) => {}
            Err(e) => panic!("Error while checking key schedule test vector.\n{:?}", e),
        }
    }

    // FIXME: Interop #495
    // // mlspp test vectors
    // let tv_files = [
    //     "test_vectors/mlspp/mlspp_key_schedule_1.json",
    //     "test_vectors/mlspp/mlspp_key_schedule_2.json",
    //     "test_vectors/mlspp/mlspp_key_schedule_3.json",
    // ];
    // for &tv_file in tv_files.iter() {
    //     let tv: KeyScheduleTestVector = read(tv_file);
    //     run_test_vector(tv).expect("Error while checking key schedule test vector.");
    // }
}

#[cfg(any(feature = "test-utils", test))]
pub fn run_test_vector(test_vector: KeyScheduleTestVector) -> Result<(), KsTestVectorError> {
    use tls_codec::{Deserialize, Serialize};

    use crate::ciphersuite::HpkePublicKey;

    let ciphersuite =
        CiphersuiteName::try_from(test_vector.cipher_suite).expect("Invalid ciphersuite");
    let ciphersuite = match Config::ciphersuite(ciphersuite) {
        Ok(cs) => cs,
        Err(_) => {
            log::info!(
                "Unsupported ciphersuite {} in test vector. Skipping ...",
                ciphersuite
            );
            return Ok(());
        }
    };
    let crypto = OpenMlsRustCrypto::default();
    log::debug!("Testing test vector for ciphersuite {:?}", ciphersuite);
    log::trace!("  {:?}", test_vector);

    let group_id = hex_to_bytes(&test_vector.group_id);
    let init_secret = hex_to_bytes(&test_vector.initial_init_secret);
    log::trace!(
        "  InitSecret from tve: {:?}",
        test_vector.initial_init_secret
    );
    let mut init_secret = InitSecret::from(Secret::from_slice(
        &init_secret,
        ProtocolVersion::default(),
        ciphersuite,
    ));

    for (i, epoch) in test_vector.epochs.iter().enumerate() {
        log::debug!("  Epoch {:?}", i);
        let tree_hash = hex_to_bytes(&epoch.tree_hash);
        let commit_secret = hex_to_bytes(&epoch.commit_secret);
        let commit_secret = CommitSecret::from(Secret::from_slice(
            &commit_secret,
            ProtocolVersion::default(),
            ciphersuite,
        ));
        log::trace!("    CommitSecret from tve {:?}", epoch.commit_secret);
        let mut psks = Vec::new();
        let mut psk_ids = Vec::new();
        for psk_value in epoch.psks.iter() {
            psk_ids.push(
                PreSharedKeyId::tls_deserialize(&mut hex_to_bytes(&psk_value.psk_id).as_slice())
                    .unwrap(),
            );
            psks.push(Secret::from_slice(
                &hex_to_bytes(&psk_value.psk),
                ProtocolVersion::default(),
                ciphersuite,
            ));
        }
        // let psk = Vec::new();
        let psk_secret = PskSecret::new(ciphersuite, &crypto, &psk_ids, &psks).unwrap();

        let joiner_secret = JoinerSecret::new(&crypto, &commit_secret, &init_secret);
        if hex_to_bytes(&epoch.joiner_secret) != joiner_secret.as_slice() {
            if cfg!(test) {
                panic!("Joiner secret mismatch");
            }
            return Err(KsTestVectorError::JoinerSecretMismatch);
        }

        let mut key_schedule = KeySchedule::init(
            ciphersuite,
            &crypto,
            joiner_secret.clone(),
            Some(PskSecret::from(psk_secret)),
        );
        let welcome_secret = key_schedule.welcome(&crypto).unwrap();

        if hex_to_bytes(&epoch.welcome_secret) != welcome_secret.as_slice() {
            if cfg!(test) {
                panic!("Welcome secret mismatch");
            }
            return Err(KsTestVectorError::WelcomeSecretMismatch);
        }

        let confirmed_transcript_hash = hex_to_bytes(&epoch.confirmed_transcript_hash);

        let group_context = GroupContext::new(
            GroupId::from_slice(&group_id),
            GroupEpoch(i as u64),
            tree_hash.to_vec(),
            confirmed_transcript_hash.clone(),
            &[], // Extensions
        )
        .expect("Error creating group context");

        let expected_group_context = hex_to_bytes(&epoch.group_context);
        let group_context_serialized = group_context.tls_serialize_detached().unwrap();
        if group_context_serialized != expected_group_context {
            log::error!("  Group context mismatch");
            log::debug!("    Computed: {:x?}", group_context_serialized);
            log::debug!("    Expected: {:x?}", expected_group_context);
            if cfg!(test) {
                panic!("Group context mismatch");
            }
            return Err(KsTestVectorError::GroupContextMismatch);
        }

        key_schedule.add_context(&crypto, &group_context).unwrap();

        let epoch_secrets = key_schedule.epoch_secrets(&crypto, true).unwrap();

        init_secret = epoch_secrets.init_secret().unwrap().clone();
        if hex_to_bytes(&epoch.init_secret) != init_secret.as_slice() {
            log_crypto!(
                debug,
                "    Epoch secret mismatch: {:x?} != {:x?}",
                hex_to_bytes(&epoch.init_secret),
                init_secret.as_slice()
            );
            if cfg!(test) {
                panic!("Init secret mismatch");
            }
            return Err(KsTestVectorError::InitSecretMismatch);
        }
        if hex_to_bytes(&epoch.sender_data_secret) != epoch_secrets.sender_data_secret().as_slice()
        {
            if cfg!(test) {
                panic!("Sender data secret mismatch");
            }
            return Err(KsTestVectorError::SenderDataSecretMismatch);
        }
        if hex_to_bytes(&epoch.encryption_secret) != epoch_secrets.encryption_secret().as_slice() {
            if cfg!(test) {
                panic!("Encryption secret mismatch");
            }
            return Err(KsTestVectorError::EncryptionSecretMismatch);
        }
        if hex_to_bytes(&epoch.exporter_secret) != epoch_secrets.exporter_secret().as_slice() {
            if cfg!(test) {
                panic!("Exporter secret mismatch");
            }
            return Err(KsTestVectorError::ExporterSecretMismatch);
        }
        if hex_to_bytes(&epoch.authentication_secret)
            != epoch_secrets.authentication_secret().as_slice()
        {
            if cfg!(test) {
                panic!("Authentication secret mismatch");
            }
            return Err(KsTestVectorError::AuthenticationSecretMismatch);
        }
        if hex_to_bytes(&epoch.external_secret) != epoch_secrets.external_secret().as_slice() {
            if cfg!(test) {
                panic!("External secret mismatch");
            }
            return Err(KsTestVectorError::ExternalSecretMismatch);
        }
        if hex_to_bytes(&epoch.confirmation_key) != epoch_secrets.confirmation_key().as_slice() {
            if cfg!(test) {
                panic!("Confirmation key mismatch");
            }
            return Err(KsTestVectorError::ConfirmationKeyMismatch);
        }
        if hex_to_bytes(&epoch.membership_key) != epoch_secrets.membership_key().as_slice() {
            if cfg!(test) {
                panic!("Membership key mismatch");
            }
            return Err(KsTestVectorError::MembershipKeyMismatch);
        }
        if hex_to_bytes(&epoch.resumption_secret) != epoch_secrets.resumption_secret().as_slice() {
            if cfg!(test) {
                panic!("Resumption secret mismatch");
            }
            return Err(KsTestVectorError::ResumptionSecretMismatch);
        }

        // Calculate external HPKE key pair
        let external_key_pair = epoch_secrets
            .external_secret()
            .derive_external_keypair(crypto.crypto(), ciphersuite);
        if hex_to_bytes(&epoch.external_pub)
            != HpkePublicKey::from(external_key_pair.public.clone())
                .tls_serialize_detached()
                .unwrap()
        {
            log::error!("  External public key mismatch");
            log::debug!(
                "    Computed: {:x?}",
                HpkePublicKey::from(external_key_pair.public)
                    .tls_serialize_detached()
                    .unwrap()
            );
            log::debug!("    Expected: {:x?}", hex_to_bytes(&epoch.external_pub));
            if cfg!(test) {
                panic!("External pub mismatch");
            }
            return Err(KsTestVectorError::ExternalPubMismatch);
        }
    }
    Ok(())
}