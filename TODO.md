
- [ ] using rusqlite, create database.rs, with Database struct that creates if not exists sqlite db, and implements all relevant functions to import, delete symlink items
    - for each new item, in the database create an id (if nesseary) and the added date (current timestamp)
    - when added symlink is already in db , skip
    - when whole system search didnt find symlinks that are in the db, delete them from the db
    - create a function to find all symlinks from db that points/targets given path
    - when a symlink is in db but hasnt the same broken status, update db accordingly

- [ ] create cli.rs using Clap that creates a cli with commands sync and import that lets cli user to add a symlink into db by passing its path

- [ ] update walker 
    - make sure target of found symlink is contained in root (maybe better to do it inside symlink.rs as an error)
    - add walker to search only in subroot (make sure that subroot is a subfolder of root)
