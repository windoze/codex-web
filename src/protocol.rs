pub mod event_msg {
    use typify::import_types;

    import_types!(schema = "schemas/codex_app_server_protocol.v2.schemas.json");
}

pub mod jsonrpc {
    use typify::import_types;

    import_types!(schema = "schemas/JSONRPCMessage.json");
}
