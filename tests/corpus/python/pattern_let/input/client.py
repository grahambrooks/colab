def sync(client, user_id):
    profile = client.getUserProfile(user_id)
    client.updateHTTPHeaders({"x": 1})
    client.close()
    return profile.displayName
