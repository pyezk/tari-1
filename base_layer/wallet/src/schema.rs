table! {
    client_key_values (key) {
        key -> Text,
        value -> Text,
    }
}

table! {
    completed_transactions (tx_id) {
        tx_id -> BigInt,
        source_public_key -> Binary,
        destination_public_key -> Binary,
        amount -> BigInt,
        fee -> BigInt,
        transaction_protocol -> Text,
        status -> Integer,
        message -> Text,
        timestamp -> Timestamp,
        cancelled -> Integer,
        direction -> Nullable<Integer>,
        coinbase_block_height -> Nullable<BigInt>,
        send_count -> Integer,
        last_send_timestamp -> Nullable<Timestamp>,
        valid -> Integer,
        confirmations -> Nullable<BigInt>,
        mined_height -> Nullable<BigInt>,
        unique_id -> Nullable<Binary>,
    }
}

table! {
    contacts (public_key) {
        public_key -> Binary,
        alias -> Text,
    }
}

table! {
    inbound_transactions (tx_id) {
        tx_id -> BigInt,
        source_public_key -> Binary,
        amount -> BigInt,
        receiver_protocol -> Text,
        message -> Text,
        timestamp -> Timestamp,
        cancelled -> Integer,
        direct_send_success -> Integer,
        send_count -> Integer,
        last_send_timestamp -> Nullable<Timestamp>,
    }
}

table! {
    key_manager_states (id) {
        id -> Nullable<BigInt>,
        master_key -> Binary,
        branch_seed -> Text,
        primary_key_index -> BigInt,
        timestamp -> Timestamp,
    }
}

table! {
    known_one_sided_payment_scripts (script_hash) {
        script_hash -> Binary,
        private_key -> Binary,
        script -> Binary,
        input -> Binary,
    }
}

table! {
    outbound_transactions (tx_id) {
        tx_id -> BigInt,
        destination_public_key -> Binary,
        amount -> BigInt,
        fee -> BigInt,
        sender_protocol -> Text,
        message -> Text,
        timestamp -> Timestamp,
        cancelled -> Integer,
        direct_send_success -> Integer,
        send_count -> Integer,
        last_send_timestamp -> Nullable<Timestamp>,
        unique_id -> Nullable<Binary>,
    }
}

table! {
    outputs (id) {
        id -> Integer,
        commitment -> Binary,
        spending_key -> Binary,
        value -> BigInt,
        flags -> Integer,
        maturity -> BigInt,
        status -> Integer,
        tx_id -> Nullable<BigInt>,
        hash -> Binary,
        script -> Binary,
        input_data -> Binary,
        height -> BigInt,
        script_private_key -> Binary,
        script_offset_public_key -> Binary,
        unique_id -> Nullable<Binary>,
    }
}

table! {
    pending_transaction_outputs (tx_id) {
        tx_id -> BigInt,
        short_term -> Integer,
        timestamp -> Timestamp,
        coinbase_block_height -> Nullable<BigInt>,
    }
}

table! {
    wallet_settings (key) {
        key -> Text,
        value -> Text,
    }
}

allow_tables_to_appear_in_same_query!(
    client_key_values,
    completed_transactions,
    contacts,
    inbound_transactions,
    key_manager_states,
    known_one_sided_payment_scripts,
    outbound_transactions,
    outputs,
    pending_transaction_outputs,
    wallet_settings,
);
