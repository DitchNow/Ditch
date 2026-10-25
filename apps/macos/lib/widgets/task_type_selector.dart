import 'package:flutter/material.dart';
import '../data/task_templates.dart';

class TaskTypeSelector extends StatelessWidget {
  const TaskTypeSelector({
    required this.value,
    required this.onChanged,
    super.key,
  });
  final String value;
  final ValueChanged<String>? onChanged;

  @override
  Widget build(BuildContext context) => DropdownButtonFormField<String>(
    key: ValueKey('task-type-$value'),
    initialValue: value,
    decoration: const InputDecoration(labelText: 'Type'),
    items: [
      for (final kind in taskIssueKinds)
        DropdownMenuItem(value: kind, child: Text(kind)),
    ],
    onChanged: onChanged == null
        ? null
        : (value) {
            if (value != null) onChanged!(value);
          },
  );
}
