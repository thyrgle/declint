def handle_get(req):
    print(req)
    raise

def setup():
    db.query("select 1")
