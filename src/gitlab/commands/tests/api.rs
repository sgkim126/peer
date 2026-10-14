use std::assert_matches;

use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use super::super::api::{self, commit_error, position_error};
use super::*;
use crate::glab::GlabRunError;

fn next_page(headers: Vec<(&str, &str)>) -> Result<Option<Url>, CommandError> {
    api::next_page(&api::Response {
        status: 200,
        headers,
        body: b"[]",
    })
}

#[tokio::test]
async fn rejects_pagination_links_to_another_origin() {
    let server = Server::start(vec![Reply::json(json!([])).header(
        "Link: <https://example.com/api/v4/projects/5/notes?page=2>; rel=\"next\"",
    )])
    .await;

    assert_matches!(
        api::list::<Value>(&client(&server), "projects/5/notes").await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_to_another_collection() {
    let server =
        Server::start(vec![Reply::json(json!([])).header(
            "Link: <{base}projects/another-project/notes?page=2>; rel=\"next\"",
        )])
        .await;

    assert_matches!(
        api::list::<Value>(&client(&server), "projects/5/notes").await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_with_fragments() {
    let server =
        Server::start(vec![Reply::json(json!([])).header(
            "Link: <{base}projects/5/notes?page=2#fragment>; rel=\"next\"",
        )])
        .await;

    assert_matches!(
        api::list::<Value>(&client(&server), "projects/5/notes").await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_pagination_links_back_to_an_already_visited_page() {
    let server = Server::start(vec![
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?per_page=100>; rel=\"next\""),
        Reply::json(json!([])),
    ])
    .await;

    assert_matches!(
        api::list::<Value>(&client(&server), "projects/5/notes").await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn rejects_repeated_next_page_links() {
    let server = Server::start(vec![
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
        Reply::json(json!([])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
    ])
    .await;

    assert_matches!(
        api::list::<Value>(&client(&server), "projects/5/notes").await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn rejects_next_links_without_an_angle_bracketed_url() {
    assert_matches!(
        next_page(vec![("Link", "not-a-url; rel=next")]),
        Err(CommandError::InvalidPagination)
    );
}

#[test]
fn rejects_relative_next_page_links() {
    assert_matches!(
        next_page(vec![("Link", "<relative-path>; rel=next")]),
        Err(CommandError::InvalidPagination)
    );
}

#[test]
fn rejects_multiple_next_page_links() {
    assert_matches!(
        next_page(vec![(
            "Link",
            "<https://gitlab.com/api/v4/one>; rel=next, <https://gitlab.com/api/v4/two>; rel=next",
        )]),
        Err(CommandError::InvalidPagination)
    );
}

#[tokio::test]
async fn collects_items_from_all_linked_pages() {
    let server = Server::start(vec![
        Reply::json(json!([1, 2])).header("Link: <{base}projects/5/notes?page=2>; rel=\"next\""),
        Reply::json(json!([3])).header("Link: <{base}projects/5/notes?page=3>; rel=\"next\""),
        Reply::json(json!([4])),
    ])
    .await;

    let items = api::list::<Value>(&client(&server), "projects/5/notes")
        .await
        .unwrap();

    assert_eq!(items, vec![json!(1), json!(2), json!(3), json!(4)]);
    assert_eq!(server.requests().len(), 3);
    assert_eq!(
        server.invocation(0).0,
        [
            "api",
            "https://gitlab.com/api/v4/projects/5/notes?per_page=100",
            "--hostname",
            "gitlab.com",
            "--method",
            "GET",
            "--include",
            "--header",
            "Accept: application/json",
        ]
    );
    assert_matches!(server.invocation(0).1.as_slice(), []);
}

#[tokio::test]
async fn preserves_initial_query_parameters() {
    let server = Server::start(vec![Reply::json(json!([]))]).await;

    api::list::<Value>(&client(&server), "projects/5/notes?sort=asc")
        .await
        .unwrap();

    assert_eq!(
        server.invocation(0).0[1],
        "https://gitlab.com/api/v4/projects/5/notes?sort=asc&per_page=100"
    );
}

#[tokio::test]
async fn preserves_server_provided_next_urls() {
    let server = Server::start(vec![
        Reply::json(json!([1])).header(concat!(
            "Link: <{base}projects/5/notes?",
            "pagination=keyset&cursor=opaque%2Bvalue%2Fpart%3D&per_page=2&sort=desc>; rel=\"next\"",
        )),
        Reply::json(json!([2])),
    ])
    .await;

    api::list::<Value>(&client(&server), "projects/5/notes?sort=asc")
        .await
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        server.invocation(1).0[1],
        concat!(
            "https://gitlab.com/api/v4/projects/5/notes?",
            "pagination=keyset&cursor=opaque%2Bvalue%2Fpart%3D&per_page=2&sort=desc",
        )
    );
}

#[test]
fn ignores_conflicting_x_next_page() {
    let next = "https://gitlab.com/api/v4/projects/5/notes?page=2";
    let link = format!("<{next}>; rel=next");

    assert_eq!(
        next_page(vec![("Link", &link), ("X-Next-Page", "3")]).unwrap(),
        Some(Url::parse(next).unwrap())
    );
}

#[test]
fn ignores_x_next_page_without_link() {
    assert_eq!(next_page(vec![("X-Next-Page", "2")]).unwrap(), None);
}

#[test]
fn ignores_previous_page_links() {
    assert_eq!(
        next_page(vec![(
            "Link",
            "<https://gitlab.com/api/v4/projects/5/notes?page=1>; rel=prev",
        )])
        .unwrap(),
        None
    );
}

#[tokio::test]
async fn rejects_credentials_on_the_api_origin_before_sending_a_request() {
    let server = Server::start(Vec::new()).await;
    let mut url = Url::parse("https://gitlab.com/api/v4/projects/5/notes").unwrap();
    url.set_username("user").unwrap();
    url.set_password(Some("password")).unwrap();
    assert_matches!(
        api::get::<Value>(&client(&server), url.as_str()).await,
        Err(CommandError::InvalidPagination)
    );
    assert_eq!(server.requests(), Vec::<String>::new());
}

#[tokio::test]
async fn reports_redirect_responses_without_retry() {
    let server = Server::start(vec![
        Reply::json(json!({}))
            .status(302)
            .header("Location: {base}redirect"),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        api::get::<Value>(&client(&server), "projects/5").await,
        Err(CommandError::Api { status: 302, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn does_not_retry_authentication_failures() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(401),
        Reply::json(json!({})),
    ])
    .await;

    assert_matches!(
        api::get::<Value>(&client(&server), "projects/5").await,
        Err(CommandError::Api { status: 401, .. })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reports_rate_limit_without_retry() {
    let server = Server::start(vec![
        Reply::json(json!({})).status(429).header("Retry-After: 1"),
    ])
    .await;

    assert_matches!(
        api::get::<Value>(&client(&server), "projects/5").await,
        Err(CommandError::Api {
            status: 429,
            rate_limited: true,
            ..
        })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reports_access_denied_without_retry() {
    let server = Server::start(vec![
        Reply::json(json!({}))
            .status(403)
            .header("RateLimit-Remaining: 10"),
    ])
    .await;

    assert_matches!(
        api::get::<Value>(&client(&server), "projects/5").await,
        Err(CommandError::Api {
            status: 403,
            rate_limited: false,
            ..
        })
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reports_invalid_json_as_a_decode_error() {
    let mut reply = Reply::json(json!({}));
    reply.body = "invalid-json".into();
    let server = Server::start(vec![reply]).await;
    assert_matches!(
        api::get::<Value>(&client(&server), "projects/5").await,
        Err(CommandError::Decode { .. })
    );
}

#[tokio::test]
async fn reports_a_slow_response_as_a_request_timeout() {
    let mut reply = Reply::json(json!({}));
    reply.delay = Duration::from_millis(100);
    let server = Server::start(vec![reply]).await;
    let glab = server.glab(Duration::from_millis(10));
    assert_matches!(
        api::get::<Value>(&glab, "projects/5").await,
        Err(CommandError::Run(GlabRunError::Timeout))
    );
}

#[tokio::test]
async fn bad_request_responses_do_not_imply_publication() {
    let server = Server::start(vec![Reply::json(json!({})).status(400)]).await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();
    assert_matches!(error, CommandError::Api { status: 400, .. });
    assert!(!error.may_have_published());
}

#[tokio::test]
async fn request_timeout_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(408)]).await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();
    assert_matches!(error, CommandError::Api { status: 408, .. });
    assert!(error.may_have_published());
}

#[tokio::test]
async fn server_error_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(500)]).await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();
    assert_matches!(error, CommandError::Api { status: 500, .. });
    assert!(error.may_have_published());
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn undecodable_created_responses_may_have_published() {
    let server = Server::start(vec![Reply::json(json!({})).status(201)]).await;
    #[derive(Debug, Deserialize)]
    struct Posted {
        #[serde(rename = "id")]
        _id: u64,
    }
    let error = api::post::<Posted>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();
    assert_matches!(error, CommandError::Decode { .. });
    assert!(error.may_have_published());
}

#[test]
fn recognizes_position_field_validation_errors() {
    let message = json!({
        "position": [
            "is invalid"
        ]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_dotted_position_validation_errors() {
    let message = json!({
        "position.head_sha": [
            "is invalid"
        ]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_errors_with_multiple_position_fields() {
    let message = json!({
        "position": [
            "is invalid"
        ],
        "line_code": [
            "can't be blank"
        ]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_bracketed_position_validation_errors() {
    let message = json!({
        "position[head_sha]": "is invalid"
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_nested_position_validation_errors() {
    let message = json!({
        "position": {
            "new_line": [
                "is invalid"
            ]
        }
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_line_code_validation_errors() {
    let message = json!({
        "line_code": [
            "can't be blank"
        ]
    });

    assert!(position_error(&message));
}

#[test]
fn recognizes_commit_id_errors_that_mention_diff_refs() {
    let message = json!({
        "commit_id": [
            "does not match the diff refs"
        ]
    });

    assert!(position_error(&message));
}

#[test]
fn rejects_empty_position_error_objects() {
    let message = json!({});

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_fields_with_empty_validation_arrays() {
    let message = json!({
        "position": []
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_fields_with_null_validation_details() {
    let message = json!({
        "position": null
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_mixed_position_and_body_validation_errors() {
    let message = json!({
        "position": [
            "is invalid"
        ],
        "body": [
            "can't be blank"
        ]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_body_errors_that_only_mention_position() {
    let message = json!({
        "body": [
            "contains the word position"
        ]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_field_names_that_only_start_with_position() {
    let message = json!({
        "positioning": [
            "is invalid"
        ]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_commit_id_errors_without_diff_refs() {
    let message = json!({
        "commit_id": [
            "is invalid"
        ]
    });

    assert!(!position_error(&message));
}

#[test]
fn rejects_position_error_messages_in_arrays() {
    let message = json!(["position is invalid"]);

    assert!(!position_error(&message));
}

#[test]
fn rejects_plain_text_position_validation_errors() {
    let message = json!("position is invalid");

    assert!(!position_error(&message));
}

#[test]
fn recognizes_commit_target_string_validation_messages() {
    let message = json!({
        "commit_id": "is invalid"
    });

    assert!(commit_error(&message));
}

#[test]
fn recognizes_multiple_commit_target_validation_messages() {
    let message = json!({
        "commit_id": [
            "is invalid",
            "does not exist"
        ]
    });

    assert!(commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_body_errors() {
    let message = json!({
        "commit_id": [
            "is invalid"
        ],
        "body": [
            "is invalid"
        ]
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_mixed_with_position_errors() {
    let message = json!({
        "commit_id": [
            "is invalid"
        ],
        "position": [
            "is invalid"
        ]
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_with_empty_message_arrays() {
    let message = json!({
        "commit_id": []
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_commit_target_errors_with_mixed_blank_and_nonblank_messages() {
    let message = json!({
        "commit_id": [
            "is invalid",
            " "
        ]
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_empty_commit_target_error_objects() {
    let message = json!({});

    assert!(!commit_error(&message));
}

#[test]
fn rejects_field_names_that_only_start_with_commit_id() {
    let message = json!({
        "commit_id_suffix": [
            "is invalid"
        ]
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_body_errors_that_only_mention_commit_id() {
    let message = json!({
        "body": [
            "commit_id is invalid"
        ]
    });

    assert!(!commit_error(&message));
}

#[test]
fn rejects_plain_text_commit_target_validation_errors() {
    let message = json!("commit_id is invalid");

    assert!(!commit_error(&message));
}

#[tokio::test]
async fn bad_request_responses_can_report_commit_target_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "commit_id": [
                    "is invalid"
                ]
            }
        }))
        .status(400),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/merge_requests/1/discussions",
        &json!({
            "body": "A note",
            "commit_id": "abc1234"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            status: 400,
            position_invalid: false,
            commit_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn forbidden_responses_are_not_commit_target_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "commit_id": [
                    "is invalid"
                ]
            }
        }))
        .status(403),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/merge_requests/1/discussions",
        &json!({
            "body": "A note",
            "commit_id": "abc1234"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            status: 403,
            position_invalid: false,
            commit_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn mixed_target_and_body_response_errors_do_not_permit_fallback() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "commit_id": [
                    "is invalid"
                ],
                "body": [
                    "can't be blank"
                ]
            }
        }))
        .status(422),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/merge_requests/1/discussions",
        &json!({
            "body": "A note",
            "commit_id": "abc1234"
        }),
    )
    .await;

    assert_matches!(
        error,
        Err(CommandError::Api {
            position_invalid: false,
            commit_invalid: false,
            ..
        })
    );
}

#[tokio::test]
async fn bad_request_responses_can_report_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [
                    "is invalid"
                ]
            }
        }))
        .status(400),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            position_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn unprocessable_entity_responses_can_report_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [
                    "is invalid"
                ]
            }
        }))
        .status(422),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            position_invalid: true,
            ..
        }
    );
}

#[tokio::test]
async fn forbidden_responses_are_not_position_failures() {
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [
                    "is invalid"
                ]
            }
        }))
        .status(403),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn malformed_error_json_is_not_a_position_failure() {
    let mut reply = Reply::json(json!({})).status(400);
    reply.body = "invalid-json".into();
    let server = Server::start(vec![reply]).await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert_matches!(
        error,
        CommandError::Api {
            position_invalid: false,
            ..
        }
    );
}

#[tokio::test]
async fn api_error_debug_omits_malformed_response_bodies() {
    let secret = "private-response-message";
    let mut reply = Reply::json(json!({})).status(400);
    reply.body = format!("invalid-json {secret}");
    let server = Server::start(vec![reply]).await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert!(!format!("{error:?}").contains(secret));
}

#[tokio::test]
async fn api_error_display_omits_position_validation_messages() {
    let secret = "private-response-message";
    let server = Server::start(vec![
        Reply::json(json!({
            "message": {
                "position": [
                    secret
                ]
            }
        }))
        .status(400),
    ])
    .await;
    let error = api::post::<Value>(
        &client(&server),
        "projects/5/notes",
        &json!({
            "body": "A note"
        }),
    )
    .await
    .unwrap_err();

    assert!(!error.to_string().contains(secret));
}
