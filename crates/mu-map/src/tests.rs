use super::*;

fn info(json: &str) -> RoomInfo {
    RoomInfo::parse(json).expect("valid Room.Info")
}

#[test]
fn parses_the_common_room_info_shapes() {
    let ire = info(r#"{"num": 12, "name": "Town Square", "area": "Town", "coords": "45,2,-1,0", "exits": {"n": 13, "e": "14"}}"#);
    assert_eq!(ire.id.as_deref(), Some("12"));
    assert_eq!(ire.position, Some((2.0 * GRID_X, -GRID_Y)));
    assert_eq!(ire.exits, vec![("n".to_string(), Some("13".to_string())), ("e".to_string(), Some("14".to_string()))]);

    let aard = info(r#"{"num": 32418, "name": "Inn", "zone": "aylor", "coord": {"id": 0, "x": 3, "y": 4, "cont": 0}, "exits": {"s": 32417}}"#);
    assert_eq!(aard.area.as_deref(), Some("aylor"));
    assert_eq!(aard.position, Some((3.0 * GRID_X, 4.0 * GRID_Y)));

    let beip = info(r#"{"area": "Arx", "name": "Gate", "coords": {"x": 100, "y": 40}, "size": {"x": 80, "y": 30}}"#);
    assert_eq!((beip.id, beip.position, beip.size), (None, Some((100.0, 40.0)), Some((80.0, 30.0))));

    assert!(RoomInfo::parse("[]").is_none());
}

#[test]
fn automaps_with_server_ids_and_links_exits_later() {
    let mut atlas = Atlas::default();
    let a = atlas.apply_room_info(&info(r#"{"num": 1, "name": "A", "area": "Z", "exits": {"n": 2}}"#), None, None).unwrap();
    // The exit north is known, but its room isn't mapped yet.
    assert_eq!(atlas.room(a).unwrap().exits[0].to, None);
    // Walking north: no coordinates, so B is placed north of A.
    let b = atlas.apply_room_info(&info(r#"{"num": 2, "name": "B", "area": "Z", "exits": {"s": 1}}"#), Some(a), Some("north")).unwrap();
    let (ra, rb) = (atlas.room(a).unwrap(), atlas.room(b).unwrap());
    assert_eq!((rb.x, rb.y), (ra.x, ra.y - GRID_Y));
    assert_eq!(ra.exit("n").unwrap().to, Some(b), "A's waiting exit was linked");
    assert_eq!(rb.exit("south").unwrap().to, Some(a));
    // Seeing the same room again doesn't duplicate it.
    assert_eq!(atlas.apply_room_info(&info(r#"{"num": 1, "name": "A"}"#), Some(b), Some("s")), Some(a));
    assert_eq!(atlas.len(), 2);
}

#[test]
fn rooms_without_ids_are_matched_by_name_and_area() {
    let mut atlas = Atlas::default();
    let gate = atlas.apply_room_info(&info(r#"{"area": "Arx", "name": "Gate", "coords": {"x": 0, "y": 0}}"#), None, None);
    let again = atlas.apply_room_info(&info(r#"{"area": "Arx", "name": "Gate", "coords": {"x": 50, "y": 0}}"#), None, None);
    assert_eq!(gate, again);
    assert_eq!(atlas.room(gate.unwrap()).unwrap().x, 50.0, "BeipMU coordinates move the room");
    // A move without exits from the server still records the exit taken.
    let road = atlas.apply_room_info(&info(r#"{"area": "Arx", "name": "Road"}"#), gate, Some("e")).unwrap();
    assert_eq!(atlas.room(gate.unwrap()).unwrap().exit("east").unwrap().to, Some(road));
}

#[test]
fn placement_avoids_overlaps() {
    let mut atlas = Atlas::default();
    let a = atlas.add_room("A", "", Some((0.0, 0.0)));
    let b = atlas.add_room_from(a, "B", "east", None).unwrap();
    let c = atlas.add_room_from(a, "C", "east", None).unwrap();
    let (rb, rc) = (atlas.room(b).unwrap(), atlas.room(c).unwrap());
    assert_eq!((rb.x, rb.y), (GRID_X, 0.0));
    assert!((rc.x, rc.y) != (rb.x, rb.y), "the second room east goes somewhere free");
    assert_eq!(atlas.room(b).unwrap().exit("w").unwrap().to, Some(a), "the way back defaults to the opposite");
}

#[test]
fn shortest_paths() {
    let mut atlas = Atlas::default();
    let a = atlas.add_room("A", "", Some((0.0, 0.0)));
    let b = atlas.add_room_from(a, "B", "e", None).unwrap();
    let c = atlas.add_room_from(b, "C", "e", None).unwrap();
    let d = atlas.add_room_from(c, "D", "n", None).unwrap();
    // A shortcut from A straight to D.
    atlas.set_exit(a, "enter portal", Some(d), None);
    assert_eq!(atlas.path(a, c), Some(vec!["e".to_string(), "e".to_string()]));
    assert_eq!(atlas.path(a, d), Some(vec!["enter portal".to_string()]));
    assert_eq!(atlas.path(d, a), Some(vec!["s".to_string(), "w".to_string(), "w".to_string()]));
    assert_eq!(atlas.path(a, a), Some(vec![]));
    let island = atlas.add_room("Island", "", None);
    assert_eq!(atlas.path(a, island), None);
}

#[test]
fn live_tracking_by_room_name() {
    let mut atlas = Atlas::default();
    let hall = atlas.add_room("Hall", "", Some((0.0, 0.0)));
    let kitchen = atlas.add_room_from(hall, "Kitchen", "n", None).unwrap();
    let closet = atlas.add_room_from(hall, "Closet", "e", None).unwrap();
    assert_eq!(atlas.track(hall, "  Kitchen ", None), Some(kitchen));
    assert_eq!(atlas.track(hall, "Closet", Some("e")), Some(closet));
    assert_eq!(atlas.track(hall, "Somewhere else", Some("n")), None);
    // Only neighbours count.
    assert_eq!(atlas.track(kitchen, "Closet", None), None);
}

#[test]
fn editing_and_search() {
    let mut atlas = Atlas::default();
    let a = atlas.add_room("Town Square", "T", Some((0.0, 0.0)));
    let b = atlas.add_room_from(a, "Square Fountain", "n", None).unwrap();
    // No exact match, so shorter names first.
    assert_eq!(atlas.search("square"), vec![a, b]);
    assert_eq!(atlas.search("town square"), vec![a]);
    assert_eq!(atlas.room_at("T", 10.0, 10.0), Some(a));
    atlas.rename_room(a, "Plaza");
    atlas.move_room(a, 500.0, 500.0);
    assert_eq!(atlas.room_at("T", 10.0, 10.0), None);
    atlas.delete_room(b);
    assert!(atlas.room(a).unwrap().exits.is_empty(), "exits into a deleted room go too");
    assert_eq!(atlas.areas(), vec!["T".to_string()]);
}

#[test]
fn save_and_load() {
    let dir = std::env::temp_dir().join(format!("mu-map-test-{}", std::process::id()));
    let path = dir.join("world.json");
    let mut atlas = Atlas::default();
    let a = atlas.add_room("A", "Z", None);
    atlas.add_room_from(a, "B", "s", Some("n"));
    atlas.save(&path).unwrap();
    assert!(!atlas.is_dirty());
    let loaded = Atlas::load(&path).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded.path(a, a + 1), Some(vec!["s".to_string()]));
    // New ids continue after the loaded ones.
    let mut loaded = loaded;
    assert_eq!(loaded.add_room("C", "Z", None), 3);
    assert_eq!(Atlas::load(&dir.join("missing.json")).unwrap().len(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn directions() {
    assert_eq!(canonical("North"), "n");
    assert_eq!(canonical(" Enter Portal "), "enter portal");
    assert_eq!(reverse("southwest"), Some("ne"));
    assert_eq!(reverse("portal"), None);
    assert_eq!(offset("up"), Some((0.5, -1.0)));
}
