Release Checklist
-----------------

* Run `cargo fmt`

* Edit the `Cargo.toml` to set the new xaskpass version.

* Run `cargo update`

* Run `cargo outdated` and review semver incompatible updates. Unless there is a strong motivation otherwise, review and update every dependency.

* Update default configuration file.

* Update man version.

* Commit the changes and create a new signed tag.
