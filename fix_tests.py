import re

with open('contracts/subscription_vault/src/test.rs', 'r') as f:
    content = f.read()

# Replace 6-argument calls with 7-argument calls by injecting `&None, ` before the last argument
# We find `client.create_subscription` or `client.try_create_subscription`
# and match the 6 arguments explicitly.
pattern = r'(client\.(?:try_)?create_subscription\s*\(\s*[^,]+,\s*[^,]+,\s*[^,]+,\s*[^,]+,\s*[^,]+,)(\s*[^)]+\s*\))'

# In some cases the function call might span multiple lines
# But we can just inject `&None, ` before the final argument
content = re.sub(pattern, r'\1 &None,\2', content)

with open('contracts/subscription_vault/src/test.rs', 'w') as f:
    f.write(content)
