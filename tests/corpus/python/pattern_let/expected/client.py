def sync(client, user_id):
    profile = client.get_user_profile(user_id)
    client.update_http_headers({"x": 1})
    client.close()
    return profile.displayName
