// @generated automatically by Diesel CLI.

diesel::table! {
    jobs (id) {
        id -> VarChar,
        disk -> VarChar,
        name -> VarChar,
        log -> Text,
    }
}
