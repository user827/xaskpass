Release Checklist
-----------------

* Run `cargo fmt`

* Run `cargo fix`

* Edit the `Cargo.toml` to set the new xaskpass version.

* Run `cargo update`

* Run `cargo outdated` and review semver incompatible updates. Unless there is a strong motivation otherwise, review and update every dependency.

* Update default configuration file.

* Commit the changes and create a new signed tag with release notes.

* Check the load time
