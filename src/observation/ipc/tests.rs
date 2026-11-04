use super::*;

#[test]
fn publication_shutdown_unlocks_even_with_an_inherited_descriptor() {
    let state = std::env::temp_dir().join(crate::ledger::new_id());
    let publisher = super::super::Publisher::new().unwrap();
    let server = Server::bind(&state, publisher.subscribe()).unwrap();
    // A duplicated descriptor shares the open-file description, as an inherited fork
    // descriptor does before exec. Closing only the parent's file cannot release its lock.
    let inherited = server.lock.try_clone().unwrap();
    drop(server);
    let restarted = Server::bind(&state, publisher.subscribe());
    drop(inherited);
    let server = restarted.unwrap();
    drop(server);
    std::fs::remove_dir_all(state).unwrap();
}
