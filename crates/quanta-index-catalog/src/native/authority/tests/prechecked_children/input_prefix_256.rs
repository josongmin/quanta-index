//! The bounded input prefix must retain child-first and duplicate decisions at 256.

use super::*;
use quanta_index_core::index::ObjectInstallScope;
use quanta_index_core::index::ports::ManagedObjectInstallPort;

fn reference(index: u16) -> Result<ObjectRef, IndexError> {
    let mut bytes = [0_u8; 32];
    bytes
        .get_mut(..2)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&index.to_be_bytes());
    *bytes.get_mut(2).ok_or(IndexError::ResourceLimited)? = 0xa5;
    Ok(ObjectRef(Digest(bytes)))
}

fn packet(external: ObjectRef) -> Result<Vec<RegisteredObject>, IndexError> {
    let mut objects = Vec::new();
    for index in 0_u16..256 {
        let mut children = vec![external; 8];
        if let Some(prior) = index.checked_sub(1) {
            children.push(reference(prior)?);
        }
        objects.push(RegisteredObject::unscoped(reference(index)?, 1, children));
    }
    Ok(objects)
}

#[test]
fn complete_256_prefix_accepts_external_and_prior_children_and_rejects_last_duplicate_and_self()
-> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let external = object(193, vec![]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&external),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&external),
        &mut admission_budget(),
    )?;
    let attempt = admit_managed_attempt(&catalog, operation(12109, 179))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let objects = packet(external.reference)?;
    catalog.reserve_managed(scope, &objects, &mut admission_budget())?;
    catalog.confirm_managed(scope, &objects, &mut admission_budget())?;
    let connection = Connection::open(catalog.path())?;
    let last = objects.last().ok_or("missing last input")?;
    let payload: Vec<u8> = connection.query_row(
        "SELECT payload FROM native_object_v1 WHERE object=?1",
        [object_key(last.reference)],
        |row| row.get(0),
    )?;
    let stored = registration_codec::decode(&payload)?;
    check_eq!(stored.children, last.children);
    let prior = reference(254)?;
    check_eq!(last.children.last(), Some(&prior));

    let before = managed_install_counts(&catalog)?;
    let mut duplicate = objects.clone();
    *duplicate.last_mut().ok_or("missing duplicate slot")? =
        objects.first().ok_or("missing first input")?.clone();
    check!(matches!(
        catalog.reserve_managed(scope, &duplicate, &mut admission_budget()),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(managed_install_counts(&catalog)?, before);

    let mut self_edge = objects.clone();
    let last = self_edge.last_mut().ok_or("missing self-edge slot")?;
    *last = RegisteredObject::unscoped(last.reference, 1, vec![last.reference]);
    check!(matches!(
        catalog.reserve_managed(scope, &self_edge, &mut admission_budget()),
        Err(IndexError::InvalidInput(_))
    ));
    check_eq!(managed_install_counts(&catalog)?, before);

    let mut over = objects;
    over.push(RegisteredObject::unscoped(reference(256)?, 1, vec![]));
    check!(matches!(
        catalog.reserve_managed(scope, &over, &mut admission_budget()),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    Ok(())
}
