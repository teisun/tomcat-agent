def search(storage, term):
    return sorted(document_id for document_id, text in storage.read().items() if term in text)
