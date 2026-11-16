class IndexTask:
    def __init__(self, storage):
        self.storage = storage

    def rebuild(self, documents):
        self.storage.rebuild(documents)
