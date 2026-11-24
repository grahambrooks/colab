import { Button, ButtonProps } from './ui';

export function SaveButton(props: ButtonProps): JSX.Element {
  return (
    <Button onClick={props.onSave} kind="primary">
      Save
    </Button>
  );
}

export function Toolbar(): JSX.Element {
  return (
    <div className="toolbar">
      <SaveButton onSave={save} />
      <Button onClick={reset} />
      <Button disabled>Help</Button>
    </div>
  );
}
