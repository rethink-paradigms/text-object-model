import os
import re

with open('tests/heavy_integration.rs', 'r') as f:
    content = f.read()

content = content.replace('create_dir_all(&writer_dir)', 'create_dir_all(&*writer_dir)')
content = content.replace('create_dir_all(&reader_dir)', 'create_dir_all(&*reader_dir)')

with open('tests/heavy_integration.rs', 'w') as f:
    f.write(content)
