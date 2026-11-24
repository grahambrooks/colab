import { Button, PressableProps } from '@acme/ui';

export function SaveButton(props: PressableProps): JSX.Element {
  return (
    <Button onPress={props.onSave} kind="primary">
      Save
    </Button>
  );
}

export function Toolbar(): JSX.Element {
  return (
    <div className="toolbar">
      <SaveButton onSave={save} />
      <Button onPress={reset} />
      <Button disabled>Help</Button>
    </div>
  );
}
