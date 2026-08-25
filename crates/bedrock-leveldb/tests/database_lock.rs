use bedrock_leveldb::{Db, ErrorKind, LevelDbOpenOptions};

#[test]
fn second_writer_is_rejected_until_first_writer_closes() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let first =
        Db::open(directory.path(), LevelDbOpenOptions::default()).expect("open first writer");

    let error = match Db::open(directory.path(), LevelDbOpenOptions::default()) {
        Ok(_) => panic!("second writer must not acquire the database lock"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), ErrorKind::DatabaseLocked);

    drop(first);
    Db::open(directory.path(), LevelDbOpenOptions::default())
        .expect("writer lock must be released when the database closes");
}

#[test]
fn repair_is_rejected_while_writer_is_open() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let _writer =
        Db::open(directory.path(), LevelDbOpenOptions::default()).expect("open database writer");

    let error = Db::repair(directory.path(), LevelDbOpenOptions::default())
        .expect_err("repair must honor the database writer lock");
    assert_eq!(error.kind(), ErrorKind::DatabaseLocked);
}
