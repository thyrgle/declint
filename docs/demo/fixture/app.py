import os

def process(path):
    f = open(path)
    data = f.read()
    try:
        result = transform(data)
    except Exception:
        pass
    print(data)
    return result
