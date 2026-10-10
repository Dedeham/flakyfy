"""End to end: per-user playlist isolation, checked through Octo and directly on Navidrome.

Needs the seeded stack (./run.sh). Octo is the endpoint a listener's client uses, so the
checks go through it; Navidrome is asked too, as the source of truth.
"""
import unittest

import harness as h

OWN = {"alice": {"Alice Morning", "Alice Focus"}, "bob": {"Bob Workout"},
       "carol": {"Carol Quiet", "Carol Shared Songs"}}


def client(base, name):
    return h.Subsonic(base, name, h.USERS[name])


class SeedData(unittest.TestCase):
    def test_library_and_per_user_state_exist(self):
        for name in h.USERS:
            nd = client(h.NAVIDROME, name)
            self.assertGreaterEqual(len(nd.all_songs()), 12, "generated library is missing")
            self.assertTrue(nd.starred_ids(), f"{name} has no favorites")
            self.assertTrue(nd.call("getAlbumList2", {"type": "recent"}).get("albumList2", {}).get("album"),
                            f"{name} has no play history")


class PlaylistIsolation(unittest.TestCase):
    def names(self, base, user):
        return {p["name"] for p in client(base, user).playlists()}

    def test_each_user_sees_only_their_own_playlists(self):
        for base_name, base in (("octo", h.OCTO), ("navidrome", h.NAVIDROME)):
            for user, own in OWN.items():
                with self.subTest(via=base_name, user=user):
                    visible = self.names(base, user)
                    self.assertTrue(own <= visible, f"{user} lost a playlist via {base_name}: {visible}")
                    for other, theirs in OWN.items():
                        if other != user:
                            self.assertFalse(theirs & visible, f"{user} sees {other}'s playlist via {base_name}")

    def test_a_playlist_made_through_octo_stays_private_to_its_owner(self):
        alice, bob = client(h.OCTO, "alice"), client(h.OCTO, "bob")
        song_ids = [s["id"] for s in alice.all_songs()[:2]]
        created = alice.create_playlist("Alice Created In Test", song_ids)
        try:
            self.assertIn("Alice Created In Test", self.names(h.OCTO, "alice"))
            self.assertNotIn("Alice Created In Test", self.names(h.OCTO, "bob"))
            self.assertNotIn("Alice Created In Test", self.names(h.NAVIDROME, "carol"))
            with self.assertRaises(Exception):
                bob.playlist(created["id"])  # another user's private playlist is refused
        finally:
            alice.call("deletePlaylist", {"id": created["id"]})

    def test_deleting_one_users_playlist_leaves_others_untouched(self):
        carol, bob = client(h.OCTO, "carol"), client(h.OCTO, "bob")
        before = self.names(h.OCTO, "bob")
        made = carol.create_playlist("Carol Temp", [carol.all_songs()[0]["id"]])
        carol.call("deletePlaylist", {"id": made["id"]})
        self.assertEqual(before, self.names(h.OCTO, "bob"))
        self.assertNotIn("Carol Temp", self.names(h.OCTO, "carol"))


if __name__ == "__main__":
    unittest.main()
