# Flakyfy Family

Flakyfy Family gives the people you share your music with accounts of their own. Each member
gets a library of their own in Navidrome for the songs they ask for, beside the music you
share with them.

## Where members' music lives

By default members' music is kept next to your library, in a folder named `.flakyfy-family`
inside your music folder (the one Flakyfy downloads to and Navidrome scans). Flakyfy and Navidrome
already see that folder at the same path, so there is nothing to set up: no extra volume, no
extra folder on the host.

- Each member's songs are in `.flakyfy-family/<username>`, which is that member's own library in
  Navidrome.
- A song two members want is kept once, in `.flakyfy-family/.pool`, and linked into each
  member's folder.
- Your own library leaves the folder out. Navidrome skips folders whose names start with a
  dot, and when Family is turned on Flakyfy also adds the line `.flakyfy-family/` to the
  `.ndignore` file in your music folder, so the folder stays out even if Navidrome is set to
  scan such folders. Flakyfy only adds that one line; anything else in the file stays as it was.
- Flakyfy's own work on your library (covers, lyrics, explicit marks) leaves the folder out too.

When you turn Family on, Flakyfy saves the folder it uses as `Family:MembersPath` in its
settings. If Flakyfy later finds your music somewhere else, members' music does not move.

To keep members' music somewhere else, choose **Somewhere else** when you turn Family on and
give a full folder path. Mount that folder at the same path in both the flakyfy and navidrome
containers (`docker-compose.yml` has a commented example). Flakyfy checks that it can write
there when you turn Family on, and that Navidrome can see it when you add the first member.
Once members have music of their own, the folder stays where it is: Flakyfy never moves
members' files.

An install that already keeps members' music in `/members` goes on using it.

### Cloud and network storage

Family keeps a song two members want once on disk by linking it into each member's folder.
Some storage cannot keep links. Family works either way: it then stores a copy for each member
who wants the song, and the Family page says what to change.

- **rclone mounts:** add `--vfs-links` to the rclone mount that holds your music, then turn
  Family off and on so Flakyfy checks again.
- **SMB or CIFS network shares:** add the `mfsymlinks` mount option to the share.
- **NFS:** links work, nothing to change.
