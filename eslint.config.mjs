import tseslint from 'typescript-eslint';
import hooks from 'eslint-plugin-react-hooks';

export default [
  { ignores: ['**/dist/**', '**/admin-dist/**', '**/node_modules/**'] },
  {
    files: ['frontend/src/**/*.{ts,tsx}'],
    languageOptions: { parser: tseslint.parser, parserOptions: { ecmaFeatures: { jsx: true } } },
    plugins: { 'react-hooks': hooks },
    rules: {
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'error',
      'no-dupe-else-if': 'error',
      'no-unreachable': 'error',
      'no-constant-binary-expression': 'error',
      'no-unsafe-finally': 'error',
    },
  },
];
