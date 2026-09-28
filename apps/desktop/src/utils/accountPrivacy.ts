export function maskAccountEmail(email: string) {
  if (email.length <= 10) return "*****";
  return `${email.slice(0, 5)}*****${email.slice(-5)}`;
}
