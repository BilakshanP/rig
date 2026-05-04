import argparse


def main() -> None:
    parser = argparse.ArgumentParser(description="{{name}}")
    _ = parser.parse_args()
    print("Hello from {{name}}!")
