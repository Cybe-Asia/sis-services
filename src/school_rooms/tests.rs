use super::*;
fn failed(f: Failure) -> Box<dyn std::error::Error + Send + Sync> {
    format!("failure {}", f.0).into()
}
fn room() -> Room {
    Room {
        tenant_id: "TENANT-001".into(),
        school_id: "SCH-IISS".into(),
        id: "room-7a".into(),
        name: "Room 7A".into(),
        status: "active".into(),
    }
}
#[test]
fn room_validation() {
    assert!(room().valid());
    assert!(Room {
        status: "archived".into(),
        ..room()
    }
    .valid());
    for bad in [
        Room {
            id: "Room-7A".into(),
            ..room()
        },
        Room {
            id: "room 7a".into(),
            ..room()
        },
        Room {
            name: " Room".into(),
            ..room()
        },
        Room {
            name: String::new(),
            ..room()
        },
        Room {
            name: "a\u{7}b".into(),
            ..room()
        },
        Room {
            name: "x".repeat(121),
            ..room()
        },
        Room {
            status: "deleted".into(),
            ..room()
        },
        Room {
            school_id: "SCH|X".into(),
            ..room()
        },
    ] {
        assert!(!bad.valid(), "{bad:?}");
    }
}
#[test]
fn change_rejects_unknown_fields() {
    let ok = json!({"room":{"tenantId":"T","schoolId":"S","id":"r","name":"R","status":"active"},"expectedVersion":0});
    assert!(serde_json::from_value::<Change>(ok).is_ok());
    let extra = json!({"room":{"tenantId":"T","schoolId":"S","id":"r","name":"R","status":"active","capacity":30},"expectedVersion":0});
    assert!(serde_json::from_value::<Change>(extra).is_err());
}
/// Isolated graph only: SCHOOL_ROOM_TEST_NEO4J_URI (+ _USER/_PASSWORD) and a `room-test-<uuid>` mark.
#[tokio::test]
#[ignore = "explicit isolated SCHOOL_ROOM_TEST_NEO4J_URI only"]
async fn catalogue_and_meeting_join() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mark = std::env::var("SCHOOL_ROOM_TEST_MARK")?;
    assert!(
        mark.starts_with("room-test-")
            && uuid::Uuid::parse_str(mark.trim_start_matches("room-test-")).is_ok()
    );
    let graph = Graph::new(
        std::env::var("SCHOOL_ROOM_TEST_NEO4J_URI")?,
        std::env::var("SCHOOL_ROOM_TEST_NEO4J_USER")?,
        std::env::var("SCHOOL_ROOM_TEST_NEO4J_PASSWORD")?,
    )
    .await?;
    init(&graph).await?;
    let (school, tenant, owner, stranger) = (
        format!("{mark}-school"),
        mark.clone(),
        format!("{mark}-owner"),
        format!("{mark}-stranger"),
    );
    graph.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,room_test:$mark}) CREATE(:StaffMember {id:$owner,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],room_test:$mark}) CREATE(:StaffMember {id:$stranger,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],room_test:$mark}) CREATE(:StaffMember {id:$teacher,displayName:'Synthetic Teacher',room_test:$mark}) CREATE(:LearningClassMeeting {key:$meeting,id:'m1',school_id:$school,tenant_id:$tenant,class_id:'c1',teacher_id:$teacher,room_id:'room-7a',starts_at:1,status:'published',payload:'{\"id\":\"m1\",\"roomId\":\"room-7a\"}',room_test:$mark})").param("school",school.clone()).param("tenant",tenant.clone()).param("owner",owner.clone()).param("stranger",stranger.clone()).param("teacher",format!("{mark}-teacher")).param("meeting",format!("{mark}-meeting")).param("mark",mark.clone())).await?;
    let result = async {
        let mut r = Room {
            tenant_id: tenant.clone(),
            school_id: school.clone(),
            ..room()
        };
        let change = |room: &Room, v| Change {
            room: room.clone(),
            expected_version: v,
        };
        assert_eq!(
            save(&graph, &stranger, &change(&r, 0))
                .await
                .map_err(failed)?,
            None,
            "non-administrator"
        );
        assert_eq!(
            save(&graph, &owner, &change(&r, 0)).await.map_err(failed)?,
            Some(1)
        );
        assert_eq!(
            save(&graph, &owner, &change(&r, 0)).await.map_err(failed)?,
            None,
            "create twice"
        );
        r.name = "Room 7A (Lab)".into();
        assert_eq!(
            save(&graph, &owner, &change(&r, 1)).await.map_err(failed)?,
            Some(2)
        );
        let listed = rooms(&graph, &owner, &tenant, &school)
            .await
            .map_err(failed)?;
        assert_eq!(listed["items"][0]["name"], "Room 7A (Lab)");
        assert_eq!(listed["items"][0]["version"], 2);
        let join = |graph: Graph| {
            let (school, tenant) = (school.clone(), tenant.clone());
            async move {
                let mut rows = graph
                    .execute(
                        query(crate::learning_access::class_meetings::READ)
                            .param("school", school)
                            .param("tenant", tenant)
                            .param("class", "c1")
                            .param("id", "").param("from", None::<i64>).param("to", None::<i64>),
                    )
                    .await?;
                let row = rows.next().await?.ok_or("meeting")?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
                    row.get::<Option<String>>("teacher_name")?,
                    row.get::<Option<String>>("room_name")?,
                ))
            }
        };
        assert_eq!(
            join(graph.clone()).await?,
            (
                Some("Synthetic Teacher".into()),
                Some("Room 7A (Lab)".into())
            )
        );
        r.status = "archived".into();
        assert_eq!(
            save(&graph, &owner, &change(&r, 2)).await.map_err(failed)?,
            Some(3)
        );
        assert_eq!(
            join(graph.clone()).await?.1,
            None,
            "archived rooms are not shown"
        );
        let audits = graph
            .execute(
                query("MATCH(a:SchoolRoomAudit {school_id:$school}) RETURN count(a) AS n")
                    .param("school", school.clone()),
            )
            .await?
            .next()
            .await?
            .ok_or("audit")?
            .get::<i64>("n")?;
        assert_eq!(audits, 3);
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    graph.run(query("MATCH(n) WHERE n.room_test=$mark OR (n:SchoolRoom AND n.school_id=$school) OR (n:SchoolRoomAudit AND n.school_id=$school) OR (n:EducationCalendarLock AND n.key=$lock) DETACH DELETE n").param("mark",mark.clone()).param("school",school.clone()).param("lock",format!("{tenant}|{school}"))).await?;
    result
}
