# Local document index

Python 3, standard library only. Run `python3 -m unittest -v`.

Storage is a UTF-8 JSON object mapping document IDs to text. `IndexTask.rebuild` is currently synchronous. Queries read the on-disk index on every call; startup opens an existing index or creates an empty one. The tests describe current behavior, including an interrupted rebuild. There is no external service.
