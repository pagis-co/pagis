# Mobile screen validation

The captures use Chromium, the committed reference sample data, Inter, a 390 px phone viewport, and device scale factor 2. Home also has a 390 × 1180 scroll capture and a dark-theme capture. Native controls and the Desk video use fixtures.

The capture inventory records 39 reference states with no horizontal overflow and no unexpected page errors. The Connect browser capture records one expected missing-native-plugin error. The breakpoint results cover 24 routes at widths 360, 760, 761 and 1280 px.

See [captures](screens.json), [breakpoints](breakpoints.json) and [check results](checks.json).

The full edit gate does not pass in this cloud environment. The check results name each environment limit. Browser captures do not verify native rendering or a real Computer video stream.

## Focused Rust results

```text
PASS [   0.144s] ( 1/88) pagis::main channels::a_request_without_a_session_is_rejected_and_a_session_is_accepted
PASS [   0.171s] ( 2/88) pagis::main channels::channel_list_returns_created_channels
PASS [   0.210s] ( 3/88) pagis::main channels::replies_are_one_level_deep
PASS [   0.255s] ( 4/88) pagis::main channels::duplicate_pending_id_returns_the_original_message
PASS [   0.165s] ( 5/88) pagis::main channels::send_and_read_back_through_the_timeline
PASS [   0.191s] ( 6/88) pagis::main channels::send_validation_and_unknown_channel
PASS [   0.208s] ( 7/88) pagis::main channels::timeline_pages_newest_first_with_cursor
PASS [   0.151s] ( 8/88) pagis::main connect::installation_client::a_callback_without_the_transaction_cookie_spends_the_state
PASS [   0.221s] ( 9/88) pagis::main connect::installation_client::a_callback_without_the_transaction_cookie_connects_nothing
PASS [   0.375s] (10/88) pagis::main channels::sourced_chat_rereads_hide_revoked_text_in_every_projection
PASS [   0.193s] (11/88) pagis::main connect::installation_client::a_consent_from_an_unverified_address_stores_no_token
PASS [   0.229s] (12/88) pagis::main connect::installation_client::a_new_consent_from_another_google_account_stores_no_token
PASS [   0.196s] (13/88) pagis::main connect::installation_client::a_replayed_state_stores_no_token_and_clears_the_cookie
PASS [   0.163s] (14/88) pagis::main connect::installation_client::a_result_that_grants_no_requested_capability_stores_no_token
PASS [   0.379s] (15/88) pagis::main connect::installation_client::a_granted_agent_reaches_the_account_that_consented
PASS [   0.156s] (16/88) pagis::main connect::installation_client::a_sign_out_between_start_and_callback_stores_no_token_and_clears_the_cookie
PASS [   0.178s] (17/88) pagis::main connect::installation_client::a_transaction_cookie_of_another_state_binds_nothing
PASS [   0.169s] (18/88) pagis::main connect::installation_client::an_expired_state_stores_no_token_and_clears_the_cookie
PASS [   0.187s] (19/88) pagis::main connect::installation_client::another_person_connects_the_google_account_that_one_person_connected
PASS [   0.316s] (20/88) pagis::main connect::installation_client::an_expired_google_token_publishes_the_connection_repair_state
PASS [   0.417s] (21/88) pagis::main connect::installation_client::access_revoked_at_google_surfaces_on_the_record_and_the_next_call
PASS [   0.242s] (22/88) pagis::main connect::installation_client::in_remote_access_the_client_app_person_signs_in_in_the_system_browser_and_finishes
PASS [   0.227s] (23/88) pagis::main connect::installation_client::in_remote_access_the_start_route_requires_a_session_of_the_initiating_person
PASS [   0.150s] (24/88) pagis::main connect::installation_client::on_a_loopback_origin_the_transaction_cookie_leaves_out_secure
PASS [   0.222s] (25/88) pagis::main connect::installation_client::no_secret_reaches_an_interface_the_daemon_retains
PASS [   0.200s] (26/88) pagis::main connect::installation_client::on_a_single_person_installation_a_callback_without_the_transaction_cookie_still_connects_nothing
PASS [   0.197s] (27/88) pagis::main connect::installation_client::on_a_single_person_installation_a_sign_out_in_the_client_app_ends_the_authorization
PASS [   0.225s] (28/88) pagis::main connect::installation_client::on_a_single_person_installation_the_system_browser_goes_to_google_without_a_session
PASS [   0.208s] (29/88) pagis::main connect::installation_client::only_the_requested_capabilities_that_google_granted_are_recorded
PASS [   0.185s] (30/88) pagis::main connect::installation_client::the_initiating_browser_finishes_the_consent_and_reaches_connected
PASS [   0.171s] (31/88) pagis::main connect::installation_client::the_start_route_sets_the_transaction_cookie_and_redirects_to_google
PASS [   0.128s] (32/88) pagis::main connect::installation_client::with_the_installation_client_the_google_entry_asks_for_nothing
PASS [   0.141s] (33/88) pagis::main connect::installation_client::two_connections_get_distinct_aliases_and_a_duplicate_is_refused
PASS [   0.005s] (34/88) pagis-coding person::tests::a_body_with_no_command_and_no_location_is_the_title_cut_to_its_limit
PASS [   0.004s] (35/88) pagis-coding starts::tests::the_branch_is_a_slug_of_the_title
PASS [   0.006s] (36/88) pagis-connect::connect installation_client::a_capability_nobody_defines_never_reaches_the_provider
PASS [   0.005s] (37/88) pagis-connect::connect installation_client::a_connection_with_no_token_asks_for_a_new_consent
PASS [   0.007s] (38/88) pagis-connect::connect installation_client::a_google_connection_starts_with_no_account
PASS [   0.007s] (39/88) pagis-connect::connect installation_client::a_new_consent_from_another_account_stores_no_token
PASS [   0.005s] (40/88) pagis-connect::connect installation_client::a_refused_exchange_leaves_the_record_disconnected
PASS [   0.005s] (41/88) pagis-connect::connect installation_client::a_session_that_ended_finishes_nothing
PASS [   0.005s] (42/88) pagis-connect::connect installation_client::a_state_this_daemon_did_not_mint_reaches_no_record
PASS [   0.006s] (43/88) pagis-connect::connect installation_client::an_account_that_another_connection_holds_stores_no_token
PASS [   0.004s] (44/88) pagis-connect::connect installation_client::authorize_answers_the_start_route_and_does_not_wait
PASS [   0.006s] (45/88) pagis-connect::connect installation_client::one_person_revoking_google_leaves_the_other_connected
PASS [   0.005s] (46/88) pagis-connect::connect installation_client::only_a_carrier_has_a_sip_credential
PASS [   0.008s] (47/88) pagis-connect::connect installation_client::the_callback_finishes_the_connection_through_its_state
PASS [   0.451s] (48/88) pagis::main connect::installation_client::tenant_data_key::the_vault_and_the_broker_share_one_holder_of_the_tenant_data_keys
PASS [   0.007s] (49/88) pagis-connect::connect installation_client::the_daemon_mints_the_access_token_of_a_connection
PASS [   0.006s] (50/88) pagis-connect::connect installation_client::the_first_consent_records_the_account_that_consented
PASS [   0.005s] (51/88) pagis-connect::connect installation_client::the_start_route_answers_google_for_the_initiating_person_alone
PASS [   0.005s] (52/88) pagis-connect::connect installation_client::two_connections_get_distinct_aliases_and_a_duplicate_is_refused
PASS [   0.004s] (53/88) pagis-core memory_page::tests::a_block_with_a_title_alone_has_no_kind
PASS [   0.005s] (54/88) pagis-core memory_page::tests::a_page_title_is_the_front_matter_title_then_a_readable_name
PASS [   0.008s] (55/88) pagis-core memory_page::tests::a_heading_line_is_not_a_title
PASS [   0.006s] (56/88) pagis-core memory_page::tests::front_matter_gives_title_and_kind
PASS [   0.006s] (57/88) pagis-core run_title::tests::each_trigger_names_its_run
PASS [   0.004s] (58/88) pagis-core::subject_page fact_file_words_read_the_stem_the_title_the_aliases_and_the_first_paragraph
PASS [   0.004s] (59/88) pagis-google oauth::tests::a_capability_is_granted_when_it_was_requested_and_its_scope_came_back
PASS [   0.525s] (60/88) pagis::main connect::installation_client::tenant_data_key::concurrent_first_uses_through_the_vault_and_the_broker_seal_with_one_stored_key
PASS [   0.005s] (61/88) pagis-google oauth::tests::a_missing_or_unreadable_id_token_proves_no_account
PASS [   0.007s] (62/88) pagis-google oauth::tests::a_capability_set_becomes_the_google_scopes_that_cover_it
PASS [   0.003s] (63/88) pagis-google oauth::tests::a_web_client_needs_both_values
PASS [   0.004s] (64/88) pagis-google oauth::tests::an_audience_list_must_name_this_client
PASS [   0.004s] (65/88) pagis-google oauth::tests::an_address_google_did_not_verify_proves_no_account
PASS [   0.003s] (66/88) pagis-google oauth::tests::an_id_token_for_another_client_proves_no_account
PASS [   0.004s] (67/88) pagis-google oauth::tests::the_id_token_names_the_verified_account_for_this_client
PASS [   0.004s] (68/88) pagis-google oauth::tests::the_authorization_url_carries_the_state_the_pkce_challenge_and_offline_access
PASS [   0.004s] (69/88) pagis-google oauth::tests::the_debug_form_of_an_answer_shows_no_token
PASS [   0.004s] (70/88) pagis-google oauth::tests::the_pkce_challenge_hashes_its_verifier
PASS [   0.004s] (71/88) pagis-google oauth::tests::with_no_account_the_person_picks_one_at_google
PASS [   0.004s] (72/88) pagis-google oauth::tests::the_redirect_uri_hangs_off_the_public_origin_once
PASS [   0.007s] (73/88) pagis-google::main oauth::a_withdrawn_grant_reads_as_reauth_required
PASS [   0.007s] (74/88) pagis-google::main oauth::a_refresh_mints_an_access_token_and_keeps_the_refresh_token
PASS [   0.008s] (75/88) pagis-google::main oauth::google_being_down_is_temporary
PASS [   0.006s] (76/88) pagis-google::main oauth::the_code_is_traded_for_a_refresh_token_with_the_pkce_verifier
PASS [   0.006s] (77/88) pagis-server coding_session_feed::tests::no_event_holds_the_text_of_a_row_or_the_title
PASS [   0.006s] (78/88) pagis-server coding_sessions::tests::a_tool_call_gives_its_title
PASS [   0.006s] (79/88) pagis-server needs_you::tests::an_approval_carries_its_title_and_body_from_the_payload
PASS [   0.016s] (80/88) pagis-memory::store list_pages_searches_title_and_path_when_entity_words_are_null
PASS [   0.006s] (81/88) pagis-server notifications::tests::the_title_is_the_name_of_the_agent_or_pagis_for_an_item_with_no_agent
PASS [   0.085s] (82/88) pagis-storage-sqlite::main migrations::run_titles_are_backfilled_without_losing_references
PASS [   0.564s] (83/88) pagis::main memory::the_pages_of_a_scope_carry_the_title_kind_and_last_change
PASS [   0.516s] (84/88) pagis-testkit::stores knowledge::on_sqlite::a_forget_leaves_no_path_or_title_of_the_purged_page_in_the_feed_or_a_brief
PASS [   0.264s] (85/88) pagis-testkit::stores memory_pages::on_postgres::search_orders_a_title_match_before_a_body_match
PASS [   0.074s] (86/88) pagis-testkit::stores memory_pages::on_sqlite::search_orders_a_title_match_before_a_body_match
PASS [   0.722s] (87/88) pagis-storage-postgres::main migrations::run_titles_are_backfilled_without_losing_references
PASS [   0.732s] (88/88) pagis-testkit::stores knowledge::on_postgres::a_forget_leaves_no_path_or_title_of_the_purged_page_in_the_feed_or_a_brief
Summary [   2.831s] 88 tests run: 88 passed, 3681 skipped
```

## Final phone, token and primitive checks

```text
Test Files  3 passed (3)
Tests  45 passed (45)
```
