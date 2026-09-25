import os
from collections import *

def fetch(url):
    print("fetching", url)
    if url == None:
        return []
    return os.path.join(url)

def process(items, cache = []):
	try:
		seen = set();
		return [i for i in items if i in cache]
	except:
		return cache

print("script starts")
