import os
import re

port_counter = 8700

def process_file(filepath):
    global port_counter
    with open(filepath, 'r') as f:
        content = f.read()

    new_content = []
    lines = content.split('\n')
    for line in lines:
        if 'Runtime::open(&runtime_dir)' in line or 'Runtime::open(&writer_dir)' in line or 'Runtime::open(&reader_dir)' in line:
            var_match = re.search(r'Runtime::open\(&([a-zA-Z_]+)\)', line)
            if var_match:
                dir_var = var_match.group(1)
                indent = len(line) - len(line.lstrip())
                new_content.append(' ' * indent + f'std::fs::create_dir_all(&{dir_var}).unwrap();')
                new_content.append(' ' * indent + f'std::fs::write({dir_var}.join("config.json"), "{{\\"pandoc_port\\": {port_counter}}}\\n").unwrap();')
                port_counter += 1
        new_content.append(line)

    with open(filepath, 'w') as f:
        f.write('\n'.join(new_content))

for root, _, files in os.walk('tests'):
    for file in files:
        if file.endswith('.rs'):
            process_file(os.path.join(root, file))

print("Patched all tests.")
