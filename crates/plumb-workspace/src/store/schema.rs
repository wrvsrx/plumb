diesel::table! {
    cache_meta (key) {
        key -> Text,
        value -> BigInt,
    }
}

diesel::table! {
    documents (path) {
        path -> Binary,
        revision -> BigInt,
        content_hash -> Binary,
        valid -> Bool,
        title -> Text,
        title_start -> BigInt,
        title_end -> BigInt,
    }
}

diesel::table! {
    anchors (path, start) {
        path -> Binary,
        id -> Text,
        start -> BigInt,
        record -> Binary,
    }
}

diesel::table! {
    links (path, start) {
        path -> Binary,
        start -> BigInt,
        end -> BigInt,
        record -> Binary,
    }
}

diesel::table! {
    semantic_references (source_path, source_start, target_path) {
        source_path -> Binary,
        target_path -> Binary,
        target_id -> Nullable<Text>,
        source_start -> BigInt,
        source_end -> BigInt,
        path_start -> Nullable<BigInt>,
        path_end -> Nullable<BigInt>,
        id_start -> Nullable<BigInt>,
        id_end -> Nullable<BigInt>,
    }
}

diesel::table! {
    tasks (path, start) {
        path -> Binary,
        id -> Nullable<Text>,
        title -> Text,
        start -> BigInt,
        record -> Binary,
    }
}

diesel::table! {
    events (path, start) {
        path -> Binary,
        title -> Text,
        start -> BigInt,
        is_point -> Bool,
        sort_millis -> Nullable<BigInt>,
        interval_start_millis -> Nullable<BigInt>,
        interval_end_millis -> Nullable<BigInt>,
        record -> Binary,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    cache_meta,
    documents,
    anchors,
    links,
    semantic_references,
    tasks,
    events,
);
